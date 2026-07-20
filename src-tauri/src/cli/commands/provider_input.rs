// Provider Add/Edit 命令的共享输入逻辑
// 提供可复用的交互式输入函数，供 add 和 edit 命令使用

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::cli::ui::info;
use crate::error::AppError;
use crate::provider::{
    AuthBinding, AuthBindingSource, ClaudeApiKeyField, CodexChatReasoningConfig, Provider,
    ProviderMeta,
};
use crate::services::ProviderService;
use clap::ValueEnum;
use colored::Colorize;
use inquire::{Confirm, Select, Text};
use serde_json::{json, Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum ProviderAddTemplate {
    Custom,
    ClaudeOfficial,
    CodexOauth,
    OpenaiOfficial,
    GoogleOauth,
    Claudeapi,
    Packycode,
    Runapi,
    Aicodemirror,
    Cubence,
    Dds,
    Qiniu,
    Fenno,
    Deepseek,
}

impl ProviderAddTemplate {
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::ClaudeOfficial => "claude-official",
            Self::CodexOauth => "codex-oauth",
            Self::OpenaiOfficial => "openai-official",
            Self::GoogleOauth => "google-oauth",
            Self::Claudeapi => "claudeapi",
            Self::Packycode => "packycode",
            Self::Runapi => "runapi",
            Self::Aicodemirror => "aicodemirror",
            Self::Cubence => "cubence",
            Self::Dds => "dds",
            Self::Qiniu => "qiniu",
            Self::Fenno => "fenno",
            Self::Deepseek => "deepseek",
        }
    }

    pub fn is_custom(self) -> bool {
        matches!(self, Self::Custom)
    }

    pub fn requires_settings_prompt(self) -> bool {
        matches!(
            self,
            Self::Claudeapi
                | Self::Packycode
                | Self::Runapi
                | Self::Aicodemirror
                | Self::Cubence
                | Self::Dds
                | Self::Qiniu
                | Self::Fenno
                | Self::Deepseek
        )
    }
}

const DEEPSEEK_CODEX_CONFIG: &str = r#"model_provider = "custom"
model = "deepseek-v4-flash"
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.custom]
name = "deepseek"
base_url = "https://api.deepseek.com"
wire_api = "responses"
requires_openai_auth = true"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderAddTemplateChoice {
    pub template: ProviderAddTemplate,
    pub label: &'static str,
}

#[derive(Debug, Clone)]
pub struct SettingsConfigPromptResult {
    pub settings_config: Value,
    pub claude_api_key_field: Option<ClaudeApiKeyField>,
}

impl SettingsConfigPromptResult {
    fn new(settings_config: Value) -> Self {
        Self {
            settings_config,
            claude_api_key_field: None,
        }
    }

    fn claude(settings_config: Value, api_key_field: ClaudeApiKeyField) -> Self {
        Self {
            settings_config,
            claude_api_key_field: Some(api_key_field),
        }
    }
}

pub fn supports_common_config(app_type: &AppType) -> bool {
    matches!(app_type, AppType::Claude | AppType::Codex | AppType::Gemini)
}

pub fn common_snippet_has_effective_config(
    app_type: &AppType,
    common_snippet: Option<&str>,
) -> bool {
    if !supports_common_config(app_type) {
        return false;
    }

    let snippet = common_snippet.map(str::trim).unwrap_or_default();
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

pub fn provider_uses_common_config(
    app_type: &AppType,
    provider: &Provider,
    common_snippet: Option<&str>,
) -> bool {
    if !supports_common_config(app_type) {
        return false;
    }

    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.apply_common_config)
        .unwrap_or_else(|| {
            ProviderService::provider_uses_common_config_for_app(app_type, provider, common_snippet)
        })
}

pub fn set_provider_common_config_meta(provider: &mut Provider, enabled: bool) {
    provider
        .meta
        .get_or_insert_with(ProviderMeta::default)
        .apply_common_config = Some(enabled);
}

#[derive(Debug, Clone, Copy)]
struct SponsorProviderPreset {
    id: ProviderAddTemplate,
    provider_name: &'static str,
    chip_label: &'static str,
    website_url: &'static str,
    partner_promotion_key: &'static str,
    claude_base_url: &'static str,
    codex_base_url: &'static str,
    gemini_base_url: &'static str,
    opencode_base_url: &'static str,
    openclaw_base_url: &'static str,
    hermes_base_url: &'static str,
}

const SPONSOR_PROVIDER_PRESETS: [SponsorProviderPreset; 8] = [
    SponsorProviderPreset {
        id: ProviderAddTemplate::Claudeapi,
        provider_name: "ClaudeAPI",
        chip_label: "* ClaudeAPI",
        website_url: "https://console.claudeapi.com",
        partner_promotion_key: "claudeapi",
        claude_base_url: "https://gw.claudeapi.com",
        codex_base_url: "",
        gemini_base_url: "",
        opencode_base_url: "",
        openclaw_base_url: "",
        hermes_base_url: "",
    },
    SponsorProviderPreset {
        id: ProviderAddTemplate::Packycode,
        provider_name: "PackyCode",
        chip_label: "* PackyCode",
        website_url: "https://www.packyapi.com",
        partner_promotion_key: "packycode",
        claude_base_url: "https://www.packyapi.com",
        codex_base_url: "https://www.packyapi.com/v1",
        gemini_base_url: "https://www.packyapi.com",
        opencode_base_url: "https://www.packyapi.com/v1",
        openclaw_base_url: "https://www.packyapi.com",
        hermes_base_url: "https://www.packyapi.com",
    },
    SponsorProviderPreset {
        id: ProviderAddTemplate::Cubence,
        provider_name: "Cubence",
        chip_label: "* Cubence",
        website_url: "https://cubence.com",
        partner_promotion_key: "cubence",
        claude_base_url: "https://api.cubence.com",
        codex_base_url: "https://api.cubence.com/v1",
        gemini_base_url: "https://api.cubence.com",
        opencode_base_url: "https://api.cubence.com/v1",
        openclaw_base_url: "https://api.cubence.com",
        hermes_base_url: "https://api.cubence.com",
    },
    SponsorProviderPreset {
        id: ProviderAddTemplate::Runapi,
        provider_name: "RunAPI",
        chip_label: "* RunAPI",
        website_url: "https://runapi.co",
        partner_promotion_key: "runapi",
        claude_base_url: "https://runapi.co",
        codex_base_url: "https://runapi.co/v1",
        gemini_base_url: "",
        opencode_base_url: "https://runapi.co",
        openclaw_base_url: "https://runapi.co",
        hermes_base_url: "https://runapi.co",
    },
    SponsorProviderPreset {
        id: ProviderAddTemplate::Aicodemirror,
        provider_name: "AICodeMirror",
        chip_label: "* AICodeMirror",
        website_url: "https://www.aicodemirror.com",
        partner_promotion_key: "aicodemirror",
        claude_base_url: "https://api.aicodemirror.com/api/claudecode",
        codex_base_url: "https://api.aicodemirror.com/api/codex/backend-api/codex",
        gemini_base_url: "https://api.aicodemirror.com/api/gemini",
        opencode_base_url: "https://api.aicodemirror.com/api/claudecode",
        openclaw_base_url: "https://api.aicodemirror.com/api/claudecode",
        hermes_base_url: "",
    },
    SponsorProviderPreset {
        id: ProviderAddTemplate::Dds,
        provider_name: "DDS",
        chip_label: "* DDS",
        website_url: "https://www.ddshub.cc",
        partner_promotion_key: "dds",
        claude_base_url: "https://www.ddshub.cc",
        codex_base_url: "https://www.ddshub.cc",
        gemini_base_url: "",
        opencode_base_url: "",
        openclaw_base_url: "",
        hermes_base_url: "",
    },
    SponsorProviderPreset {
        id: ProviderAddTemplate::Qiniu,
        provider_name: "Qiniu",
        chip_label: "* Qiniu",
        website_url: "https://s.qiniu.com/FVfiEb",
        partner_promotion_key: "qiniu",
        claude_base_url: "https://api.qnaigc.com",
        codex_base_url: "https://api.qnaigc.com/bypass/openai/v1",
        gemini_base_url: "https://api.qnaigc.com/bypass/vertex",
        opencode_base_url: "https://api.qnaigc.com/v1",
        openclaw_base_url: "https://api.qnaigc.com/v1",
        hermes_base_url: "https://api.qnaigc.com/v1",
    },
    SponsorProviderPreset {
        id: ProviderAddTemplate::Fenno,
        provider_name: "FennoAI",
        chip_label: "* FennoAI",
        website_url: "https://api.fenno.ai",
        partner_promotion_key: "fenno",
        claude_base_url: "https://api.fenno.ai",
        codex_base_url: "https://api.fenno.ai",
        gemini_base_url: "",
        opencode_base_url: "https://api.fenno.ai/v1",
        openclaw_base_url: "https://api.fenno.ai/v1",
        hermes_base_url: "https://api.fenno.ai/v1",
    },
];

const PROVIDER_TEMPLATE_CHOICES_CLAUDE: [ProviderAddTemplateChoice; 11] = [
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Custom,
        label: "Custom",
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::ClaudeOfficial,
        label: "Claude Official",
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::CodexOauth,
        label: "Codex",
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Claudeapi,
        label: SPONSOR_PROVIDER_PRESETS[0].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Qiniu,
        label: SPONSOR_PROVIDER_PRESETS[6].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Fenno,
        label: SPONSOR_PROVIDER_PRESETS[7].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Runapi,
        label: SPONSOR_PROVIDER_PRESETS[3].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Cubence,
        label: SPONSOR_PROVIDER_PRESETS[2].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Packycode,
        label: SPONSOR_PROVIDER_PRESETS[1].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Aicodemirror,
        label: SPONSOR_PROVIDER_PRESETS[4].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Dds,
        label: SPONSOR_PROVIDER_PRESETS[5].chip_label,
    },
];

const PROVIDER_TEMPLATE_CHOICES_CODEX: [ProviderAddTemplateChoice; 10] = [
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Custom,
        label: "Custom",
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::OpenaiOfficial,
        label: "OpenAI Official",
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Qiniu,
        label: SPONSOR_PROVIDER_PRESETS[6].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Fenno,
        label: SPONSOR_PROVIDER_PRESETS[7].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Runapi,
        label: SPONSOR_PROVIDER_PRESETS[3].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Cubence,
        label: SPONSOR_PROVIDER_PRESETS[2].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Packycode,
        label: SPONSOR_PROVIDER_PRESETS[1].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Aicodemirror,
        label: SPONSOR_PROVIDER_PRESETS[4].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Dds,
        label: SPONSOR_PROVIDER_PRESETS[5].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Deepseek,
        label: "DeepSeek",
    },
];

const PROVIDER_TEMPLATE_CHOICES_GEMINI: [ProviderAddTemplateChoice; 6] = [
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Custom,
        label: "Custom",
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::GoogleOauth,
        label: "Google OAuth",
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Qiniu,
        label: SPONSOR_PROVIDER_PRESETS[6].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Cubence,
        label: SPONSOR_PROVIDER_PRESETS[2].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Packycode,
        label: SPONSOR_PROVIDER_PRESETS[1].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Aicodemirror,
        label: SPONSOR_PROVIDER_PRESETS[4].chip_label,
    },
];

const PROVIDER_TEMPLATE_CHOICES_OPENCODE: [ProviderAddTemplateChoice; 6] = [
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Custom,
        label: "Custom",
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Qiniu,
        label: SPONSOR_PROVIDER_PRESETS[6].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Fenno,
        label: SPONSOR_PROVIDER_PRESETS[7].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Runapi,
        label: SPONSOR_PROVIDER_PRESETS[3].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Cubence,
        label: SPONSOR_PROVIDER_PRESETS[2].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Aicodemirror,
        label: SPONSOR_PROVIDER_PRESETS[4].chip_label,
    },
];

const PROVIDER_TEMPLATE_CHOICES_HERMES: [ProviderAddTemplateChoice; 5] = [
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Custom,
        label: "Custom",
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Qiniu,
        label: SPONSOR_PROVIDER_PRESETS[6].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Fenno,
        label: SPONSOR_PROVIDER_PRESETS[7].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Runapi,
        label: SPONSOR_PROVIDER_PRESETS[3].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Cubence,
        label: SPONSOR_PROVIDER_PRESETS[2].chip_label,
    },
];

const PROVIDER_TEMPLATE_CHOICES_OPENCLAW: [ProviderAddTemplateChoice; 6] = [
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Custom,
        label: "Custom",
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Qiniu,
        label: SPONSOR_PROVIDER_PRESETS[6].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Fenno,
        label: SPONSOR_PROVIDER_PRESETS[7].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Runapi,
        label: SPONSOR_PROVIDER_PRESETS[3].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Cubence,
        label: SPONSOR_PROVIDER_PRESETS[2].chip_label,
    },
    ProviderAddTemplateChoice {
        template: ProviderAddTemplate::Aicodemirror,
        label: SPONSOR_PROVIDER_PRESETS[4].chip_label,
    },
];

pub fn provider_add_template_choices(app_type: &AppType) -> &'static [ProviderAddTemplateChoice] {
    match app_type {
        AppType::Claude => &PROVIDER_TEMPLATE_CHOICES_CLAUDE,
        AppType::Codex => &PROVIDER_TEMPLATE_CHOICES_CODEX,
        AppType::Gemini => &PROVIDER_TEMPLATE_CHOICES_GEMINI,
        AppType::OpenCode => &PROVIDER_TEMPLATE_CHOICES_OPENCODE,
        AppType::Hermes => &PROVIDER_TEMPLATE_CHOICES_HERMES,
        AppType::OpenClaw => &PROVIDER_TEMPLATE_CHOICES_OPENCLAW,
    }
}

pub fn provider_add_template_supported(app_type: &AppType, template: ProviderAddTemplate) -> bool {
    provider_add_template_choices(app_type)
        .iter()
        .any(|choice| choice.template == template)
}

pub fn provider_add_template_supported_names(app_type: &AppType) -> String {
    provider_add_template_choices(app_type)
        .iter()
        .map(|choice| choice.template.cli_name())
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn validate_provider_add_template(
    app_type: &AppType,
    template: ProviderAddTemplate,
) -> Result<(), AppError> {
    if provider_add_template_supported(app_type, template) {
        return Ok(());
    }

    let supported = provider_add_template_supported_names(app_type);
    if crate::cli::i18n::is_chinese() {
        Err(AppError::InvalidInput(format!(
            "供应商模板 '{}' 不支持应用 {}。可用模板：{}",
            template.cli_name(),
            app_type.as_str(),
            supported
        )))
    } else {
        Err(AppError::InvalidInput(format!(
            "Provider template '{}' is not supported for {}. Supported templates: {}",
            template.cli_name(),
            app_type.as_str(),
            supported
        )))
    }
}

pub fn build_provider_from_add_template(
    app_type: &AppType,
    template: ProviderAddTemplate,
    existing_ids: &[String],
) -> Result<Provider, AppError> {
    let mut provider = build_provider_template_seed(app_type, template, existing_ids)?;
    provider.created_at = Some(current_timestamp());
    Ok(provider)
}

pub fn build_provider_template_seed(
    app_type: &AppType,
    template: ProviderAddTemplate,
    existing_ids: &[String],
) -> Result<Provider, AppError> {
    validate_provider_add_template(app_type, template)?;
    if template.is_custom() {
        return Err(AppError::InvalidInput(
            "Custom provider templates require interactive field prompts".to_string(),
        ));
    }

    let name = template_default_name(template)?;
    let id = generate_provider_id(name, existing_ids);
    let website_url = template_default_website_url(template).map(str::to_string);
    let category = template_default_category(template).map(str::to_string);
    let meta = template_default_meta(app_type, template);
    let settings_config = build_provider_template_settings_config(app_type, template)?;
    let icon = template_default_icon(template).map(str::to_string);
    let icon_color = template_default_icon_color(template).map(str::to_string);

    Ok(Provider {
        id,
        name: name.to_string(),
        settings_config,
        website_url,
        category,
        created_at: None,
        sort_index: None,
        notes: None,
        icon,
        icon_color,
        meta,
        in_failover_queue: false,
    })
}

fn template_default_name(template: ProviderAddTemplate) -> Result<&'static str, AppError> {
    Ok(match template {
        ProviderAddTemplate::ClaudeOfficial => "Claude Official",
        ProviderAddTemplate::CodexOauth => "Codex",
        ProviderAddTemplate::OpenaiOfficial => "OpenAI Official",
        ProviderAddTemplate::GoogleOauth => "Google OAuth",
        ProviderAddTemplate::Deepseek => "DeepSeek",
        ProviderAddTemplate::Claudeapi
        | ProviderAddTemplate::Packycode
        | ProviderAddTemplate::Runapi
        | ProviderAddTemplate::Aicodemirror
        | ProviderAddTemplate::Cubence
        | ProviderAddTemplate::Dds
        | ProviderAddTemplate::Qiniu
        | ProviderAddTemplate::Fenno => sponsor_preset(template)
            .map(|preset| preset.provider_name)
            .ok_or_else(|| unsupported_template_error(template))?,
        ProviderAddTemplate::Custom => return Err(unsupported_template_error(template)),
    })
}

fn template_default_website_url(template: ProviderAddTemplate) -> Option<&'static str> {
    match template {
        ProviderAddTemplate::ClaudeOfficial => Some("https://www.anthropic.com/claude-code"),
        ProviderAddTemplate::CodexOauth => Some("https://openai.com/chatgpt/pricing"),
        ProviderAddTemplate::OpenaiOfficial => Some("https://chatgpt.com/codex"),
        ProviderAddTemplate::GoogleOauth => Some("https://ai.google.dev"),
        ProviderAddTemplate::Deepseek => Some("https://platform.deepseek.com"),
        ProviderAddTemplate::Claudeapi
        | ProviderAddTemplate::Packycode
        | ProviderAddTemplate::Runapi
        | ProviderAddTemplate::Aicodemirror
        | ProviderAddTemplate::Cubence
        | ProviderAddTemplate::Dds
        | ProviderAddTemplate::Qiniu
        | ProviderAddTemplate::Fenno => sponsor_preset(template).map(|preset| preset.website_url),
        ProviderAddTemplate::Custom => None,
    }
}

fn template_default_category(template: ProviderAddTemplate) -> Option<&'static str> {
    match template {
        ProviderAddTemplate::ClaudeOfficial
        | ProviderAddTemplate::OpenaiOfficial
        | ProviderAddTemplate::GoogleOauth => Some("official"),
        ProviderAddTemplate::Deepseek => Some("cn_official"),
        ProviderAddTemplate::Runapi | ProviderAddTemplate::Qiniu | ProviderAddTemplate::Fenno => {
            Some("aggregator")
        }
        ProviderAddTemplate::Custom
        | ProviderAddTemplate::CodexOauth
        | ProviderAddTemplate::Claudeapi
        | ProviderAddTemplate::Packycode
        | ProviderAddTemplate::Aicodemirror
        | ProviderAddTemplate::Cubence
        | ProviderAddTemplate::Dds => None,
    }
}

fn template_default_meta(
    app_type: &AppType,
    template: ProviderAddTemplate,
) -> Option<ProviderMeta> {
    match template {
        ProviderAddTemplate::CodexOauth => Some(ProviderMeta {
            provider_type: Some("codex_oauth".to_string()),
            api_format: Some("openai_responses".to_string()),
            codex_fast_mode: Some(false),
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("codex_oauth".to_string()),
                account_id: None,
            }),
            ..Default::default()
        }),
        ProviderAddTemplate::OpenaiOfficial => Some(ProviderMeta {
            codex_official: Some(true),
            ..Default::default()
        }),
        ProviderAddTemplate::Deepseek => Some(ProviderMeta {
            api_format: Some("openai_chat".to_string()),
            codex_chat_reasoning: Some(CodexChatReasoningConfig {
                supports_thinking: Some(true),
                supports_effort: Some(true),
                thinking_param: Some("thinking".to_string()),
                effort_param: Some("reasoning_effort".to_string()),
                effort_value_mode: Some("deepseek".to_string()),
                output_format: Some("reasoning_content".to_string()),
            }),
            ..Default::default()
        }),
        ProviderAddTemplate::GoogleOauth => Some(ProviderMeta {
            partner_promotion_key: Some("google-official".to_string()),
            ..Default::default()
        }),
        ProviderAddTemplate::Claudeapi
        | ProviderAddTemplate::Packycode
        | ProviderAddTemplate::Runapi
        | ProviderAddTemplate::Aicodemirror
        | ProviderAddTemplate::Cubence
        | ProviderAddTemplate::Dds
        | ProviderAddTemplate::Qiniu
        | ProviderAddTemplate::Fenno => sponsor_preset(template).map(|preset| {
            let mut meta = ProviderMeta {
                is_partner: Some(true),
                partner_promotion_key: Some(preset.partner_promotion_key.to_string()),
                ..Default::default()
            };
            if matches!(app_type, AppType::Codex) {
                meta.api_format = Some("openai_responses".to_string());
            }
            meta
        }),
        ProviderAddTemplate::Custom | ProviderAddTemplate::ClaudeOfficial => None,
    }
}

fn template_default_icon(template: ProviderAddTemplate) -> Option<&'static str> {
    match template {
        ProviderAddTemplate::Deepseek => Some("deepseek"),
        ProviderAddTemplate::Runapi => Some("runapi"),
        ProviderAddTemplate::Qiniu => Some("qiniu"),
        ProviderAddTemplate::Fenno => Some("fenno"),
        ProviderAddTemplate::Custom
        | ProviderAddTemplate::ClaudeOfficial
        | ProviderAddTemplate::CodexOauth
        | ProviderAddTemplate::OpenaiOfficial
        | ProviderAddTemplate::GoogleOauth
        | ProviderAddTemplate::Claudeapi
        | ProviderAddTemplate::Packycode
        | ProviderAddTemplate::Aicodemirror
        | ProviderAddTemplate::Cubence
        | ProviderAddTemplate::Dds => None,
    }
}

fn template_default_icon_color(template: ProviderAddTemplate) -> Option<&'static str> {
    match template {
        ProviderAddTemplate::Deepseek => Some("#1E88E5"),
        ProviderAddTemplate::Custom
        | ProviderAddTemplate::ClaudeOfficial
        | ProviderAddTemplate::CodexOauth
        | ProviderAddTemplate::OpenaiOfficial
        | ProviderAddTemplate::GoogleOauth
        | ProviderAddTemplate::Claudeapi
        | ProviderAddTemplate::Packycode
        | ProviderAddTemplate::Runapi
        | ProviderAddTemplate::Aicodemirror
        | ProviderAddTemplate::Cubence
        | ProviderAddTemplate::Dds
        | ProviderAddTemplate::Qiniu
        | ProviderAddTemplate::Fenno => None,
    }
}

fn build_provider_template_settings_config(
    app_type: &AppType,
    template: ProviderAddTemplate,
) -> Result<Value, AppError> {
    match template {
        ProviderAddTemplate::ClaudeOfficial => Ok(json!({ "env": {} })),
        ProviderAddTemplate::CodexOauth => Ok(json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://chatgpt.com/backend-api/codex",
                "ANTHROPIC_MODEL": "gpt-5.4",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "gpt-5.4-mini",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "gpt-5.4",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "gpt-5.4",
            },
            "attribution": {
                "commit": "",
                "pr": ""
            }
        })),
        ProviderAddTemplate::OpenaiOfficial => build_codex_official_settings_config(None),
        ProviderAddTemplate::Deepseek => Ok(build_codex_deepseek_settings_config()),
        ProviderAddTemplate::GoogleOauth => Ok(json!({ "env": {} })),
        ProviderAddTemplate::Claudeapi
        | ProviderAddTemplate::Packycode
        | ProviderAddTemplate::Runapi
        | ProviderAddTemplate::Aicodemirror
        | ProviderAddTemplate::Cubence
        | ProviderAddTemplate::Dds
        | ProviderAddTemplate::Qiniu
        | ProviderAddTemplate::Fenno => build_sponsor_template_settings_config(
            app_type,
            sponsor_preset(template).ok_or_else(|| unsupported_template_error(template))?,
        ),
        ProviderAddTemplate::Custom => Err(unsupported_template_error(template)),
    }
}

fn build_codex_deepseek_settings_config() -> Value {
    json!({
        "config": DEEPSEEK_CODEX_CONFIG,
        "modelCatalog": {
            "models": [
                {
                    "model": "deepseek-v4-flash",
                    "displayName": "DeepSeek V4 Flash",
                    "contextWindow": 1000000,
                },
                {
                    "model": "deepseek-v4-pro",
                    "displayName": "DeepSeek V4 Pro",
                    "contextWindow": 1000000,
                },
            ],
        },
    })
}

fn runapi_opencode_settings_config(base_url: &str) -> Result<Value, AppError> {
    build_opencode_settings_config(
        Some(&json!({
            "name": "RunAPI",
            "options": {
                "setCacheKey": true,
            },
            "models": {
                "claude-opus-4-8": {
                    "name": "Claude Opus 4.8",
                },
                "claude-haiku-4-5": {
                    "name": "Claude Haiku 4.5",
                },
            },
        })),
        "@ai-sdk/anthropic",
        "",
        base_url,
        "claude-sonnet-4-6",
        "Claude Sonnet 4.6",
        "",
        "",
        None,
    )
}

fn runapi_hermes_models() -> Value {
    json!([
        {
            "id": "claude-opus-4-8",
            "name": "Claude Opus 4.8",
        },
        {
            "id": "claude-sonnet-4-6",
            "name": "Claude Sonnet 4.6",
        },
        {
            "id": "claude-haiku-4-5",
            "name": "Claude Haiku 4.5",
        },
    ])
}

fn runapi_openclaw_models() -> Value {
    json!([
        {
            "id": "claude-opus-4-8",
            "name": "Claude Opus 4.8",
            "contextWindow": 1000000,
        },
        {
            "id": "claude-sonnet-4-6",
            "name": "Claude Sonnet 4.6",
            "contextWindow": 1000000,
        },
        {
            "id": "claude-haiku-4-5",
            "name": "Claude Haiku 4.5",
            "contextWindow": 200000,
        },
    ])
}

fn qiniu_opencode_settings_config(base_url: &str) -> Result<Value, AppError> {
    build_opencode_settings_config(
        Some(&json!({
            "name": "Qiniu",
            "options": {
                "setCacheKey": true,
            },
        })),
        "@ai-sdk/openai-compatible",
        "",
        base_url,
        "gpt-5.5",
        "GPT-5.5",
        "",
        "",
        None,
    )
}

fn qiniu_hermes_models() -> Value {
    json!([
        {
            "id": "gpt-5.5",
            "name": "GPT-5.5",
        },
    ])
}

fn qiniu_openclaw_models() -> Value {
    json!([
        {
            "id": "gpt-5.5",
            "name": "GPT-5.5",
            "contextWindow": 400000,
        },
    ])
}

fn fenno_opencode_settings_config(base_url: &str) -> Result<Value, AppError> {
    build_opencode_settings_config(
        Some(&json!({
            "name": "FennoAI",
            "options": {
                "setCacheKey": true,
            },
        })),
        "@ai-sdk/openai-compatible",
        "",
        base_url,
        "gpt-5.5",
        "GPT-5.5",
        "",
        "",
        None,
    )
}

fn fenno_hermes_models() -> Value {
    json!([
        {
            "id": "gpt-5.5",
            "name": "GPT-5.5",
        },
    ])
}

fn fenno_openclaw_models() -> Value {
    json!([
        {
            "id": "gpt-5.5",
            "name": "GPT-5.5",
            "contextWindow": 400000,
        },
    ])
}

fn build_sponsor_template_settings_config(
    app_type: &AppType,
    preset: SponsorProviderPreset,
) -> Result<Value, AppError> {
    match app_type {
        AppType::Claude => Ok(json!({
            "env": {
                "ANTHROPIC_BASE_URL": preset.claude_base_url,
            }
        })),
        AppType::Codex => Ok(build_codex_settings_config(
            None,
            preset.codex_base_url,
            if matches!(
                preset.id,
                ProviderAddTemplate::Qiniu | ProviderAddTemplate::Fenno
            ) {
                "gpt-5.5"
            } else {
                "gpt-5.4"
            },
            "responses",
            preset.provider_name,
        )),
        AppType::Gemini => {
            if preset.id == ProviderAddTemplate::Qiniu {
                Ok(json!({
                    "env": {
                        "GOOGLE_GEMINI_BASE_URL": preset.gemini_base_url,
                        "GEMINI_MODEL": "gemini-3.1-pro-preview",
                    }
                }))
            } else {
                Ok(json!({
                    "env": {
                        "GOOGLE_GEMINI_BASE_URL": preset.gemini_base_url,
                    }
                }))
            }
        }
        AppType::OpenCode => {
            if preset.id == ProviderAddTemplate::Aicodemirror {
                Ok(json!({
                    "npm": "@ai-sdk/anthropic",
                    "options": {
                        "baseURL": preset.claude_base_url,
                    },
                    "models": {
                        "claude-opus-4.6": {
                            "name": "Claude Opus 4.6",
                        },
                        "claude-sonnet-4.6": {
                            "name": "Claude Sonnet 4.6",
                        },
                    },
                }))
            } else {
                if preset.id == ProviderAddTemplate::Runapi {
                    return runapi_opencode_settings_config(preset.opencode_base_url);
                }
                if preset.id == ProviderAddTemplate::Qiniu {
                    return qiniu_opencode_settings_config(preset.opencode_base_url);
                }
                if preset.id == ProviderAddTemplate::Fenno {
                    return fenno_opencode_settings_config(preset.opencode_base_url);
                }
                build_opencode_settings_config(
                    None,
                    "@ai-sdk/openai-compatible",
                    "",
                    preset.opencode_base_url,
                    "",
                    "",
                    "",
                    "",
                    None,
                )
            }
        }
        AppType::Hermes => {
            if preset.id == ProviderAddTemplate::Runapi {
                build_hermes_settings_config(
                    Some(&json!({ "name": "runapi" })),
                    "anthropic_messages",
                    preset.hermes_base_url,
                    "",
                    runapi_hermes_models(),
                    "",
                )
            } else if preset.id == ProviderAddTemplate::Qiniu {
                build_hermes_settings_config(
                    Some(&json!({ "name": "qiniu" })),
                    crate::hermes_config::HERMES_DEFAULT_API_MODE,
                    preset.hermes_base_url,
                    "",
                    qiniu_hermes_models(),
                    "",
                )
            } else if preset.id == ProviderAddTemplate::Fenno {
                build_hermes_settings_config(
                    Some(&json!({ "name": "fenno" })),
                    crate::hermes_config::HERMES_DEFAULT_API_MODE,
                    preset.hermes_base_url,
                    "",
                    fenno_hermes_models(),
                    "",
                )
            } else {
                build_hermes_settings_config(
                    None,
                    crate::hermes_config::HERMES_DEFAULT_API_MODE,
                    preset.hermes_base_url,
                    "",
                    json!([]),
                    "",
                )
            }
        }
        AppType::OpenClaw => {
            if preset.id == ProviderAddTemplate::Aicodemirror {
                build_openclaw_settings_config(
                    None,
                    "anthropic-messages",
                    "",
                    preset.claude_base_url,
                    false,
                    json!([
                        {
                            "id": "claude-opus-4-6",
                            "name": "Claude Opus 4.6",
                            "contextWindow": 200000,
                            "cost": {
                                "input": 5,
                                "output": 25,
                            },
                        },
                        {
                            "id": "claude-sonnet-4-6",
                            "name": "Claude Sonnet 4.6",
                            "contextWindow": 200000,
                            "cost": {
                                "input": 3,
                                "output": 15,
                            },
                        },
                    ]),
                )
            } else if preset.id == ProviderAddTemplate::Runapi {
                build_openclaw_settings_config(
                    None,
                    "anthropic-messages",
                    "",
                    preset.openclaw_base_url,
                    false,
                    runapi_openclaw_models(),
                )
            } else if preset.id == ProviderAddTemplate::Qiniu {
                build_openclaw_settings_config(
                    None,
                    crate::openclaw_config::OPENCLAW_DEFAULT_API_PROTOCOL,
                    "",
                    preset.openclaw_base_url,
                    false,
                    qiniu_openclaw_models(),
                )
            } else if preset.id == ProviderAddTemplate::Fenno {
                build_openclaw_settings_config(
                    None,
                    crate::openclaw_config::OPENCLAW_DEFAULT_API_PROTOCOL,
                    "",
                    preset.openclaw_base_url,
                    false,
                    fenno_openclaw_models(),
                )
            } else {
                build_openclaw_settings_config(
                    None,
                    crate::openclaw_config::OPENCLAW_DEFAULT_API_PROTOCOL,
                    "",
                    preset.openclaw_base_url,
                    false,
                    json!([{
                        "id": "placeholder-model"
                    }]),
                )
                .map(|mut value| {
                    if let Some(obj) = value.as_object_mut() {
                        obj.remove("models");
                    }
                    value
                })
            }
        }
    }
}

fn sponsor_preset(template: ProviderAddTemplate) -> Option<SponsorProviderPreset> {
    SPONSOR_PROVIDER_PRESETS
        .iter()
        .copied()
        .find(|preset| preset.id == template)
}

fn unsupported_template_error(template: ProviderAddTemplate) -> AppError {
    AppError::InvalidInput(format!(
        "Unsupported provider template '{}'",
        template.cli_name()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_official_settings_config_uses_upstream_seed_shape() {
        let cfg = build_codex_official_settings_config(None).expect("build official settings");
        assert!(
            cfg.get("auth").is_some(),
            "official Codex provider should carry auth like upstream snapshots"
        );
        assert_eq!(cfg.get("auth"), Some(&json!({})));
        assert_eq!(cfg.get("config"), Some(&json!("")));
    }

    #[test]
    fn codex_official_settings_config_preserves_auth_and_strips_provider_config() {
        let cfg = build_codex_official_settings_config(Some(&json!({
            "auth": {
                "access_token": "oauth-token",
                "refresh_token": "refresh-token"
            },
            "config": "model_provider = \"openai\"\nmodel = \"gpt-5.4\"\nmodel_reasoning_effort = \"high\"\n\n[model_providers.openai]\nbase_url = \"https://api.openai.com/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n"
        })))
        .expect("build official settings");

        assert_eq!(
            cfg.get("auth"),
            Some(&json!({
                "access_token": "oauth-token",
                "refresh_token": "refresh-token"
            }))
        );
        assert_eq!(
            cfg.get("config").and_then(Value::as_str),
            Some("model_reasoning_effort = \"high\"")
        );
    }

    #[test]
    fn build_codex_settings_config_defaults_model_to_gpt_5_4() {
        let cfg = build_codex_settings_config(
            Some("sk-test"),
            "https://api.example.com/v1",
            "",
            "responses",
            "custom",
        );

        let config = cfg
            .get("config")
            .and_then(Value::as_str)
            .expect("config should be present");
        assert!(config.contains("model = \"gpt-5.4\""));
        assert!(config.contains("base_url = \"https://api.example.com/v1\""));
    }

    #[test]
    fn cli_codex_prompt_update_uses_stable_template_model_provider_key() {
        let seed =
            build_provider_template_seed(&AppType::Codex, ProviderAddTemplate::Aicodemirror, &[])
                .expect("build AICodeMirror Codex seed");

        let cfg = build_codex_settings_config_from_prompt(
            Some(&seed.settings_config),
            "sk-updated",
            "https://codex.example/v1/",
            "gpt-6",
            "custom",
        );

        let config = cfg
            .get("config")
            .and_then(Value::as_str)
            .expect("config should be present");
        assert!(config.contains("model_provider = \"custom\""));
        assert!(config.contains("[model_providers.custom]"));
        assert!(config.contains("name = \"AICodeMirror\""));
        assert!(!config.contains("[model_providers.aicodemirror]"));
        assert!(config.contains("model = \"gpt-6\""));
        assert!(config.contains("base_url = \"https://codex.example/v1\""));
        assert_eq!(cfg["auth"]["OPENAI_API_KEY"], "sk-updated");
    }

    #[test]
    fn cli_codex_prompt_update_preserves_user_authored_model_provider_key() {
        let current = json!({
            "auth": { "OPENAI_API_KEY": "sk-old" },
            "config": r#"model_provider = "my-private-relay"
model = "gpt-5.4"

[model_providers.my-private-relay]
name = "Manual Relay"
base_url = "https://old.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
        });

        let cfg = build_codex_settings_config_from_prompt(
            Some(&current),
            "sk-updated",
            "https://new.example/v1",
            "gpt-6",
            "custom",
        );
        let config = cfg
            .get("config")
            .and_then(Value::as_str)
            .expect("config should be present");

        assert!(config.contains("model_provider = \"my-private-relay\""));
        assert!(config.contains("[model_providers.my-private-relay]"));
        assert!(!config.contains("[model_providers.custom]"));
        assert!(config.contains("base_url = \"https://new.example/v1\""));
        assert!(config.contains("model = \"gpt-6\""));
    }

    #[test]
    fn cli_codex_prompt_update_preserves_settings_siblings() {
        let current = json!({
            "auth": {
                "OPENAI_API_KEY": "sk-old",
                "OTHER_TOKEN": "keep"
            },
            "config": r#"
model_provider = "custom"
model = "gpt-5.4"

[model_providers.custom]
base_url = "https://api.old.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#,
            "modelCatalog": {
                "models": [
                    {
                        "model": "deepseek-v4-flash",
                        "name": "DeepSeek V4 Flash"
                    }
                ]
            },
            "unknownSibling": {
                "keep": true
            }
        });

        let cfg = build_codex_settings_config_from_prompt(
            Some(&current),
            "sk-updated",
            "https://api.changed.example/v1",
            "gpt-6",
            "fallback",
        );

        assert_eq!(cfg["auth"]["OPENAI_API_KEY"], "sk-updated");
        assert_eq!(cfg["auth"]["OTHER_TOKEN"], "keep");
        assert_eq!(
            cfg["modelCatalog"]["models"][0]["model"],
            "deepseek-v4-flash"
        );
        assert_eq!(cfg["unknownSibling"]["keep"], true);

        let config = cfg
            .get("config")
            .and_then(Value::as_str)
            .expect("config should be present");
        assert!(config.contains("model_provider = \"custom\""));
        assert!(config.contains("model = \"gpt-6\""));
        assert!(config.contains("base_url = \"https://api.changed.example/v1\""));
    }

    #[test]
    fn cli_gemini_api_key_settings_match_tui_env_shape() {
        let cfg = build_gemini_api_key_settings_config(
            None,
            "AIza-updated",
            "https://generativelanguage.googleapis.com",
            "gemini-3-pro-preview",
        );

        assert_eq!(cfg["env"]["GEMINI_API_KEY"], "AIza-updated");
        assert_eq!(
            cfg["env"]["GOOGLE_GEMINI_BASE_URL"],
            "https://generativelanguage.googleapis.com"
        );
        assert_eq!(cfg["env"]["GEMINI_MODEL"], "gemini-3-pro-preview");
        assert!(
            cfg.get("config").is_none(),
            "Gemini API-key settings should match TUI by omitting config"
        );
    }

    #[test]
    fn cli_gemini_oauth_settings_match_tui_empty_env_shape() {
        let cfg = build_gemini_oauth_settings_config(None);

        assert_eq!(cfg, json!({ "env": {} }));
        assert!(
            cfg.get("config").is_none(),
            "Gemini OAuth settings should not carry config or model fields"
        );
    }

    #[test]
    fn cli_gemini_api_key_settings_preserve_settings_siblings() {
        let current = json!({
            "env": {
                "GEMINI_API_KEY": "AIza-old",
                "GOOGLE_GEMINI_BASE_URL": "https://old.example",
                "GEMINI_BASE_URL": "https://legacy.example",
                "GEMINI_MODEL": "old-model",
                "EXTRA_ENV": "keep"
            },
            "config": {
                "safetySettings": [
                    {
                        "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                        "threshold": "BLOCK_NONE"
                    }
                ]
            },
            "unknownSibling": {
                "keep": true
            }
        });

        let cfg = build_gemini_api_key_settings_config(
            Some(&current),
            "AIza-updated",
            "https://generativelanguage.googleapis.com",
            "gemini-3-pro-preview",
        );

        assert_eq!(cfg["env"]["GEMINI_API_KEY"], "AIza-updated");
        assert_eq!(
            cfg["env"]["GOOGLE_GEMINI_BASE_URL"],
            "https://generativelanguage.googleapis.com"
        );
        assert_eq!(cfg["env"]["GEMINI_MODEL"], "gemini-3-pro-preview");
        assert_eq!(cfg["env"]["EXTRA_ENV"], "keep");
        assert_eq!(cfg["env"]["GEMINI_BASE_URL"], "https://legacy.example");
        assert_eq!(
            cfg["config"]["safetySettings"][0]["threshold"],
            "BLOCK_NONE"
        );
        assert_eq!(cfg["unknownSibling"]["keep"], true);
    }

    #[test]
    fn cli_gemini_oauth_settings_preserve_siblings_and_clear_gemini_env() {
        let current = json!({
            "env": {
                "GEMINI_API_KEY": "AIza-old",
                "GOOGLE_GEMINI_BASE_URL": "https://old.example",
                "GEMINI_BASE_URL": "https://legacy.example",
                "GEMINI_MODEL": "old-model",
                "EXTRA_ENV": "keep"
            },
            "config": {
                "safetySettings": []
            },
            "unknownSibling": {
                "keep": true
            }
        });

        let cfg = build_gemini_oauth_settings_config(Some(&current));

        assert!(cfg["env"].get("GEMINI_API_KEY").is_none());
        assert!(cfg["env"].get("GOOGLE_GEMINI_BASE_URL").is_none());
        assert!(cfg["env"].get("GEMINI_BASE_URL").is_none());
        assert!(cfg["env"].get("GEMINI_MODEL").is_none());
        assert_eq!(cfg["env"]["EXTRA_ENV"], "keep");
        assert_eq!(cfg["config"]["safetySettings"], json!([]));
        assert_eq!(cfg["unknownSibling"]["keep"], true);
    }

    #[test]
    fn common_config_helpers_detect_and_mark_supported_provider() {
        assert!(common_snippet_has_effective_config(
            &AppType::Claude,
            Some(r#"{"env":{"CC_SWITCH_SHARED":"1"}}"#)
        ));
        assert!(common_snippet_has_effective_config(
            &AppType::Codex,
            Some("model_reasoning_effort = \"high\"")
        ));
        assert!(!common_snippet_has_effective_config(
            &AppType::OpenCode,
            Some(r#"{"options":{"theme":"dark"}}"#)
        ));

        let mut provider = Provider::with_id(
            "p1".to_string(),
            "Provider One".to_string(),
            json!({"env": {}}),
            None,
        );
        set_provider_common_config_meta(&mut provider, true);
        assert_eq!(
            provider.meta.and_then(|meta| meta.apply_common_config),
            Some(true)
        );
    }

    #[test]
    fn cli_provider_template_labels_follow_tui_support_matrix() {
        let labels = |app_type: AppType| {
            provider_add_template_choices(&app_type)
                .iter()
                .map(|choice| choice.label)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            labels(AppType::Claude),
            vec![
                "Custom",
                "Claude Official",
                "Codex",
                "* ClaudeAPI",
                "* Qiniu",
                "* FennoAI",
                "* RunAPI",
                "* Cubence",
                "* PackyCode",
                "* AICodeMirror",
                "* DDS",
            ]
        );
        assert_eq!(
            labels(AppType::Codex),
            vec![
                "Custom",
                "OpenAI Official",
                "* Qiniu",
                "* FennoAI",
                "* RunAPI",
                "* Cubence",
                "* PackyCode",
                "* AICodeMirror",
                "* DDS",
                "DeepSeek",
            ]
        );
        assert_eq!(
            labels(AppType::Gemini),
            vec![
                "Custom",
                "Google OAuth",
                "* Qiniu",
                "* Cubence",
                "* PackyCode",
                "* AICodeMirror",
            ]
        );
        assert_eq!(
            labels(AppType::OpenCode),
            vec![
                "Custom",
                "* Qiniu",
                "* FennoAI",
                "* RunAPI",
                "* Cubence",
                "* AICodeMirror"
            ]
        );
        assert_eq!(
            labels(AppType::Hermes),
            vec!["Custom", "* Qiniu", "* FennoAI", "* RunAPI", "* Cubence"]
        );
        assert_eq!(
            labels(AppType::OpenClaw),
            vec![
                "Custom",
                "* Qiniu",
                "* FennoAI",
                "* RunAPI",
                "* Cubence",
                "* AICodeMirror"
            ]
        );
    }

    #[test]
    fn cli_provider_template_rejects_unsupported_app_template_pairs() {
        assert!(
            validate_provider_add_template(&AppType::Gemini, ProviderAddTemplate::Runapi).is_err()
        );
        assert!(
            validate_provider_add_template(&AppType::Gemini, ProviderAddTemplate::Dds).is_err()
        );
        assert!(
            validate_provider_add_template(&AppType::OpenCode, ProviderAddTemplate::Packycode)
                .is_err()
        );
        assert!(
            validate_provider_add_template(&AppType::Codex, ProviderAddTemplate::Claudeapi)
                .is_err()
        );
        assert!(
            validate_provider_add_template(&AppType::Claude, ProviderAddTemplate::GoogleOauth)
                .is_err()
        );
        assert!(
            validate_provider_add_template(&AppType::Claude, ProviderAddTemplate::Deepseek)
                .is_err()
        );
    }

    #[test]
    fn cli_provider_add_manual_id_validation_matches_tui_additive_fields() {
        let existing = vec!["taken".to_string()];

        validate_provider_id_for_add(&AppType::Hermes, "hermes-one", &existing)
            .expect("valid Hermes provider key should pass");
        assert!(
            validate_provider_id_for_add(&AppType::Hermes, "Hermes One", &existing).is_err(),
            "Hermes provider key should keep the TUI lowercase/hyphen validation"
        );
        assert!(
            validate_provider_id_for_add(&AppType::Hermes, "-bad", &existing).is_err(),
            "Hermes provider key should reject leading hyphens like the TUI"
        );
        validate_provider_id_for_add(&AppType::OpenClaw, "openclaw-provider", &existing)
            .expect("valid OpenClaw provider key should pass");
        assert!(
            validate_provider_id_for_add(&AppType::OpenClaw, "OpenClaw Provider", &existing)
                .is_err(),
            "OpenClaw provider key should keep the upstream lowercase/hyphen validation"
        );

        let err = validate_provider_id_for_add(&AppType::OpenClaw, "taken", &existing)
            .expect_err("duplicate manual provider ids should be rejected before save");
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn cli_claude_codex_oauth_template_matches_tui_contract() {
        let provider =
            build_provider_template_seed(&AppType::Claude, ProviderAddTemplate::CodexOauth, &[])
                .expect("build Codex OAuth provider");

        assert_eq!(provider.id, "codex");
        assert_eq!(provider.name, "Codex");
        assert_eq!(
            provider.website_url.as_deref(),
            Some("https://openai.com/chatgpt/pricing")
        );
        assert_eq!(
            provider.settings_config["env"]["ANTHROPIC_BASE_URL"],
            "https://chatgpt.com/backend-api/codex"
        );
        assert_eq!(
            provider.settings_config["env"]["ANTHROPIC_MODEL"],
            "gpt-5.4"
        );
        assert_eq!(
            provider.settings_config["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"],
            "gpt-5.4-mini"
        );
        assert!(
            provider.settings_config["env"]
                .get("ANTHROPIC_AUTH_TOKEN")
                .is_none(),
            "Codex OAuth providers must not persist provider API keys"
        );
        assert_eq!(provider.settings_config["attribution"]["commit"], "");
        assert_eq!(provider.settings_config["attribution"]["pr"], "");

        let meta = provider
            .meta
            .expect("Codex OAuth metadata should be present");
        assert_eq!(meta.provider_type.as_deref(), Some("codex_oauth"));
        assert_eq!(meta.api_format.as_deref(), Some("openai_responses"));
        assert_eq!(meta.codex_fast_mode, Some(false));
        let binding = meta
            .auth_binding
            .expect("Codex OAuth should bind to managed account auth");
        assert_eq!(binding.source, AuthBindingSource::ManagedAccount);
        assert_eq!(binding.auth_provider.as_deref(), Some("codex_oauth"));
        assert!(
            binding.account_id.is_none(),
            "default-account binding should omit accountId"
        );
    }

    #[test]
    fn cli_claudeapi_template_uses_claude_only_sponsor_base_url() {
        let provider =
            build_provider_template_seed(&AppType::Claude, ProviderAddTemplate::Claudeapi, &[])
                .expect("build ClaudeAPI Claude seed");

        assert_eq!(provider.id, "claudeapi");
        assert_eq!(provider.name, "ClaudeAPI");
        assert_eq!(
            provider.website_url.as_deref(),
            Some("https://console.claudeapi.com")
        );
        assert_eq!(
            provider.settings_config["env"]["ANTHROPIC_BASE_URL"],
            "https://gw.claudeapi.com"
        );
        assert_eq!(
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.partner_promotion_key.as_deref()),
            Some("claudeapi")
        );
        assert_eq!(
            provider.meta.as_ref().and_then(|meta| meta.is_partner),
            Some(true)
        );
    }

    #[test]
    fn cli_claude_sponsor_prompt_blank_api_key_omits_token() {
        let seed =
            build_provider_template_seed(&AppType::Claude, ProviderAddTemplate::Packycode, &[])
                .expect("build PackyCode Claude seed");
        let base_url = seed.settings_config["env"]["ANTHROPIC_BASE_URL"]
            .as_str()
            .expect("seed base URL should exist");

        let cfg = build_claude_settings_config_from_prompt(
            None,
            ClaudeApiKeyField::AuthToken,
            "",
            base_url,
            Vec::<(&str, Option<String>)>::new(),
            false,
        );

        assert_eq!(cfg["env"]["ANTHROPIC_BASE_URL"], "https://www.packyapi.com");
        assert!(
            cfg["env"].get("ANTHROPIC_AUTH_TOKEN").is_none(),
            "Claude sponsor prompt should match TUI by omitting blank API keys"
        );
    }

    #[test]
    fn cli_claude_prompt_api_key_field_writes_api_key_env() {
        let current = json!({
            "env": {
                "ANTHROPIC_AUTH_TOKEN": "stale-token",
                "ANTHROPIC_API_KEY": "old-api-key",
                "EXTRA_ENV": "keep"
            }
        });

        let cfg = build_claude_settings_config_from_prompt(
            Some(&current),
            ClaudeApiKeyField::ApiKey,
            "sk-updated",
            "https://api.example.com",
            Vec::<(&str, Option<String>)>::new(),
            false,
        );

        assert_eq!(cfg["env"]["ANTHROPIC_API_KEY"], "sk-updated");
        assert!(
            cfg["env"].get("ANTHROPIC_AUTH_TOKEN").is_none(),
            "switching to ANTHROPIC_API_KEY should migrate away from the default auth field"
        );
        assert_eq!(cfg["env"]["EXTRA_ENV"], "keep");
    }

    #[test]
    fn cli_claude_api_key_field_infers_meta_then_env() {
        let settings_with_api_key = json!({
            "env": {
                "ANTHROPIC_API_KEY": "sk-api-key"
            }
        });
        let meta = ProviderMeta {
            api_key_field: Some("ANTHROPIC_AUTH_TOKEN".to_string()),
            ..Default::default()
        };

        assert_eq!(
            ClaudeApiKeyField::from_meta_and_settings(Some(&meta), &settings_with_api_key),
            ClaudeApiKeyField::AuthToken,
            "upstream gives meta.apiKeyField precedence over env inference"
        );
        assert_eq!(
            ClaudeApiKeyField::from_meta_and_settings(None, &settings_with_api_key),
            ClaudeApiKeyField::ApiKey,
            "without meta, existing ANTHROPIC_API_KEY should select the API_KEY field"
        );
    }

    #[test]
    fn cli_claude_prompt_writes_hide_attribution_upstream_shape() {
        let cfg = build_claude_settings_config_from_prompt(
            None,
            ClaudeApiKeyField::AuthToken,
            "sk-test",
            "https://api.anthropic.com",
            Vec::<(&str, Option<String>)>::new(),
            true,
        );

        assert_eq!(
            cfg["attribution"],
            json!({
                "commit": "",
                "pr": ""
            })
        );
    }

    #[test]
    fn cli_claude_model_prompt_fields_cover_tui_model_config_keys() {
        let keys = claude_model_prompt_fields()
            .into_iter()
            .map(|field| field.env_key)
            .collect::<Vec<_>>();

        assert_eq!(
            keys,
            vec![
                "ANTHROPIC_MODEL",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                "ANTHROPIC_DEFAULT_SONNET_MODEL",
                "ANTHROPIC_DEFAULT_OPUS_MODEL",
            ],
            "CLI model prompts should cover the same Claude model env keys as the TUI model config"
        );
    }

    #[test]
    fn cli_claude_prompt_writes_supported_models_when_model_config_is_supplied() {
        let cfg = build_claude_settings_config_from_prompt(
            None,
            ClaudeApiKeyField::AuthToken,
            "sk-test",
            "https://api.anthropic.com",
            [
                ("ANTHROPIC_MODEL", Some("model-main".to_string())),
                (
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                    Some("model-haiku".to_string()),
                ),
                (
                    "ANTHROPIC_DEFAULT_SONNET_MODEL",
                    Some("model-sonnet".to_string()),
                ),
                (
                    "ANTHROPIC_DEFAULT_OPUS_MODEL",
                    Some("model-opus".to_string()),
                ),
            ],
            false,
        );

        assert_eq!(cfg["env"]["ANTHROPIC_MODEL"], "model-main");
        assert!(cfg["env"].get("ANTHROPIC_REASONING_MODEL").is_none());
        assert_eq!(cfg["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "model-haiku");
        assert_eq!(cfg["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"], "model-sonnet");
        assert_eq!(cfg["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "model-opus");
    }

    #[test]
    fn cli_claude_prompt_preserves_custom_attribution_when_hide_is_disabled() {
        let current = json!({
            "env": {
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-4-20250514"
            },
            "attribution": {
                "commit": "custom",
                "pr": "custom"
            },
            "extra": "keep"
        });

        let cfg = build_claude_settings_config_from_prompt(
            Some(&current),
            ClaudeApiKeyField::AuthToken,
            "sk-updated",
            "https://api.example.com",
            Vec::<(&str, Option<String>)>::new(),
            false,
        );

        assert_eq!(cfg["attribution"]["commit"], "custom");
        assert_eq!(cfg["attribution"]["pr"], "custom");
        assert_eq!(cfg["extra"], "keep");
        assert_eq!(
            cfg["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"], "claude-sonnet-4-20250514",
            "model env fields should survive when the CLI model prompt is skipped"
        );
    }

    #[test]
    fn cli_claude_prompt_preserves_existing_hidden_attribution_when_kept_enabled() {
        let current = json!({
            "env": {},
            "attribution": {
                "commit": "",
                "pr": "",
                "extra": "keep"
            }
        });

        let cfg = build_claude_settings_config_from_prompt(
            Some(&current),
            ClaudeApiKeyField::AuthToken,
            "sk-test",
            "https://api.example.com",
            Vec::<(&str, Option<String>)>::new(),
            true,
        );

        assert_eq!(
            cfg["attribution"],
            json!({
                "commit": "",
                "pr": "",
                "extra": "keep"
            })
        );
    }

    #[test]
    fn cli_claude_prompt_removes_hidden_attribution_when_disabled() {
        let current = json!({
            "env": {},
            "attribution": {
                "commit": "",
                "pr": "",
                "extra": "drop when disabled"
            }
        });

        let cfg = build_claude_settings_config_from_prompt(
            Some(&current),
            ClaudeApiKeyField::AuthToken,
            "sk-test",
            "https://api.example.com",
            Vec::<(&str, Option<String>)>::new(),
            false,
        );

        assert!(
            cfg.as_object()
                .is_some_and(|settings| !settings.contains_key("attribution")),
            "disabling hide attribution should remove the upstream hidden-attribution marker"
        );
    }

    #[test]
    fn cli_official_templates_match_tui_metadata_and_settings_shape() {
        let claude = build_provider_template_seed(
            &AppType::Claude,
            ProviderAddTemplate::ClaudeOfficial,
            &[],
        )
        .expect("build Claude Official provider");
        assert_eq!(claude.name, "Claude Official");
        assert_eq!(claude.category.as_deref(), Some("official"));
        assert_eq!(
            claude.website_url.as_deref(),
            Some("https://www.anthropic.com/claude-code")
        );
        assert_eq!(claude.settings_config, json!({ "env": {} }));
        assert!(claude.meta.is_none());

        let codex =
            build_provider_template_seed(&AppType::Codex, ProviderAddTemplate::OpenaiOfficial, &[])
                .expect("build OpenAI Official provider");
        assert_eq!(codex.name, "OpenAI Official");
        assert_eq!(codex.category.as_deref(), Some("official"));
        assert_eq!(codex.settings_config.get("auth"), Some(&json!({})));
        assert_eq!(codex.settings_config.get("config"), Some(&json!("")));
        assert_eq!(
            codex.meta.as_ref().and_then(|meta| meta.codex_official),
            Some(true)
        );

        let gemini =
            build_provider_template_seed(&AppType::Gemini, ProviderAddTemplate::GoogleOauth, &[])
                .expect("build Google OAuth provider");
        assert_eq!(gemini.name, "Google OAuth");
        assert_eq!(gemini.category.as_deref(), Some("official"));
        assert_eq!(gemini.settings_config, json!({ "env": {} }));
        assert!(gemini.settings_config.get("config").is_none());
        assert_eq!(
            gemini
                .meta
                .as_ref()
                .and_then(|meta| meta.partner_promotion_key.as_deref()),
            Some("google-official")
        );
    }

    #[test]
    fn cli_codex_deepseek_template_matches_upstream_preset_values() {
        let provider =
            build_provider_template_seed(&AppType::Codex, ProviderAddTemplate::Deepseek, &[])
                .expect("build DeepSeek Codex provider");

        assert_eq!(provider.id, "deepseek");
        assert_eq!(provider.name, "DeepSeek");
        assert_eq!(
            provider.website_url.as_deref(),
            Some("https://platform.deepseek.com")
        );
        assert_eq!(provider.category.as_deref(), Some("cn_official"));
        assert_eq!(provider.icon.as_deref(), Some("deepseek"));
        assert_eq!(provider.icon_color.as_deref(), Some("#1E88E5"));
        assert!(
            provider.settings_config.get("auth").is_none(),
            "DeepSeek preset should not persist an empty API key snapshot"
        );

        let config = provider
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .expect("DeepSeek Codex config should be TOML string");
        assert!(config.contains("model_provider = \"custom\""));
        assert!(config.contains("model = \"deepseek-v4-flash\""));
        assert!(config.contains("disable_response_storage = true"));
        assert!(config.contains("[model_providers.custom]"));
        assert!(config.contains("name = \"deepseek\""));
        assert!(config.contains("base_url = \"https://api.deepseek.com\""));
        assert!(config.contains("wire_api = \"responses\""));
        assert!(config.contains("requires_openai_auth = true"));
        assert!(
            !config.contains("https://api.deepseek.com/v1"),
            "DeepSeek Codex preset should match upstream base URL without /v1"
        );

        assert_eq!(
            provider.settings_config["modelCatalog"],
            json!({
                "models": [
                    {
                        "model": "deepseek-v4-flash",
                        "displayName": "DeepSeek V4 Flash",
                        "contextWindow": 1000000,
                    },
                    {
                        "model": "deepseek-v4-pro",
                        "displayName": "DeepSeek V4 Pro",
                        "contextWindow": 1000000,
                    },
                ],
            })
        );

        let meta = provider.meta.expect("DeepSeek metadata should be present");
        assert_eq!(meta.api_format.as_deref(), Some("openai_chat"));
        let reasoning = meta
            .codex_chat_reasoning
            .expect("DeepSeek reasoning metadata should be present");
        assert_eq!(reasoning.supports_thinking, Some(true));
        assert_eq!(reasoning.supports_effort, Some(true));
        assert_eq!(reasoning.thinking_param.as_deref(), Some("thinking"));
        assert_eq!(reasoning.effort_param.as_deref(), Some("reasoning_effort"));
        assert_eq!(reasoning.effort_value_mode.as_deref(), Some("deepseek"));
        assert_eq!(
            reasoning.output_format.as_deref(),
            Some("reasoning_content")
        );
    }

    #[test]
    fn cli_sponsor_templates_match_tui_partner_metadata_and_app_shapes() {
        let codex =
            build_provider_template_seed(&AppType::Codex, ProviderAddTemplate::Packycode, &[])
                .expect("build PackyCode Codex provider");
        let codex_config = codex
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .expect("Codex sponsor config should be TOML string");
        assert_eq!(codex.name, "PackyCode");
        assert!(codex_config.contains("base_url = \"https://www.packyapi.com/v1\""));
        assert!(codex_config.contains("model = \"gpt-5.4\""));
        assert!(codex_config.contains("wire_api = \"responses\""));
        assert_eq!(
            codex
                .meta
                .as_ref()
                .and_then(|meta| meta.partner_promotion_key.as_deref()),
            Some("packycode")
        );
        assert_eq!(
            codex.meta.as_ref().and_then(|meta| meta.is_partner),
            Some(true)
        );
        assert_eq!(
            codex
                .meta
                .as_ref()
                .and_then(|meta| meta.api_format.as_deref()),
            Some("openai_responses")
        );

        let opencode = build_provider_template_seed(
            &AppType::OpenCode,
            ProviderAddTemplate::Aicodemirror,
            &[],
        )
        .expect("build AICodeMirror OpenCode provider");
        assert_eq!(opencode.settings_config["npm"], "@ai-sdk/anthropic");
        assert_eq!(
            opencode.settings_config["options"]["baseURL"],
            "https://api.aicodemirror.com/api/claudecode"
        );
        assert_eq!(
            opencode.settings_config["models"]["claude-opus-4.6"]["name"],
            "Claude Opus 4.6"
        );
        assert_eq!(
            opencode
                .meta
                .as_ref()
                .and_then(|meta| meta.partner_promotion_key.as_deref()),
            Some("aicodemirror")
        );

        let openclaw =
            build_provider_template_seed(&AppType::OpenClaw, ProviderAddTemplate::Cubence, &[])
                .expect("build Cubence OpenClaw provider");
        assert_eq!(openclaw.settings_config["api"], "openai-completions");
        assert_eq!(
            openclaw.settings_config["baseUrl"],
            "https://api.cubence.com"
        );
        assert!(
            openclaw.settings_config.get("models").is_none(),
            "Cubence OpenClaw preset should match TUI by leaving models omitted"
        );
        assert_eq!(
            openclaw
                .meta
                .as_ref()
                .and_then(|meta| meta.partner_promotion_key.as_deref()),
            Some("cubence")
        );
    }

    #[test]
    fn build_hermes_settings_config_writes_upstream_snake_case_shape() {
        let cfg = build_hermes_settings_config(
            None,
            "anthropic_messages",
            " https://openrouter.ai/api/v1/// ",
            " sk-hermes ",
            json!([
                {
                    "id": "anthropic/claude-opus-4-7",
                    "name": "Claude Opus 4.7",
                    "context_length": 1000000
                }
            ]),
            "0.5",
        )
        .expect("build Hermes settings");

        assert_eq!(cfg["api_mode"], "anthropic_messages");
        assert_eq!(cfg["base_url"], "https://openrouter.ai/api/v1");
        assert_eq!(cfg["api_key"], "sk-hermes");
        assert_eq!(cfg["rate_limit_delay"], 0.5);
        assert_eq!(cfg["models"][0]["id"], "anthropic/claude-opus-4-7");
    }

    #[test]
    fn build_hermes_settings_config_removes_legacy_aliases_and_preserves_unknown_fields() {
        let cfg = build_hermes_settings_config(
            Some(&json!({
                "api": "openai-completions",
                "apiMode": "bedrock_converse",
                "baseUrl": "https://legacy.example/v1",
                "baseURL": "https://legacy-upper.example/v1",
                "endpoint": "https://legacy-endpoint.example/v1",
                "apiKey": "sk-legacy",
                "auth_token": "sk-auth-token",
                "key_env": "HERMES_API_KEY",
                "models": [
                    { "id": "legacy-model" }
                ]
            })),
            "",
            "",
            "",
            json!([]),
            "",
        )
        .expect("build Hermes settings");
        let obj = cfg.as_object().expect("settings object");

        assert_eq!(
            obj.get("api_mode"),
            Some(&json!(crate::hermes_config::HERMES_DEFAULT_API_MODE))
        );
        assert_eq!(obj.get("auth_token"), Some(&json!("sk-auth-token")));
        assert_eq!(obj.get("key_env"), Some(&json!("HERMES_API_KEY")));
        assert!(obj.get("base_url").is_none());
        assert!(obj.get("api_key").is_none());
        assert!(obj.get("models").is_none());
        assert!(obj.get("rate_limit_delay").is_none());
        for legacy_key in ["api", "apiMode", "baseUrl", "baseURL", "endpoint", "apiKey"] {
            assert!(
                !obj.contains_key(legacy_key),
                "Hermes save should drop legacy alias {legacy_key}"
            );
        }
    }

    #[test]
    fn build_hermes_settings_config_omits_invalid_delay_and_rejects_non_array_models() {
        let cfg = build_hermes_settings_config(None, "codex_responses", "", "", json!([]), "-1")
            .expect("build Hermes settings");
        assert_eq!(cfg["api_mode"], "codex_responses");
        assert!(cfg.get("rate_limit_delay").is_none());

        let err = build_hermes_settings_config(
            None,
            "chat_completions",
            "",
            "",
            json!({"id": "model"}),
            "",
        )
        .expect_err("non-array models should fail");
        assert!(err.to_string().contains("models"));
    }

    #[test]
    fn hermes_edit_defaults_read_legacy_aliases() {
        let defaults = HermesPromptDefaults::from_settings(Some(&json!({
            "apiMode": "bedrock_converse",
            "baseUrl": "https://legacy.example/v1",
            "apiKey": "sk-legacy",
            "auth_token": "sk-auth-token",
            "models": [
                { "id": "legacy-model", "name": "Legacy Model" }
            ],
            "rate_limit_delay": 1.25
        })));

        assert_eq!(defaults.api_mode, "bedrock_converse");
        assert_eq!(defaults.base_url, "https://legacy.example/v1");
        assert_eq!(defaults.api_key, "sk-legacy");
        assert!(defaults.models_json.contains("legacy-model"));
        assert_eq!(defaults.rate_limit_delay, "1.25");
    }

    #[test]
    fn build_opencode_settings_config_writes_tui_shape() {
        let cfg = build_opencode_settings_config(
            None,
            "",
            " sk-oc ",
            " https://api.example.com/v1 ",
            "gpt-4.1-mini",
            "GPT 4.1 Mini",
            "128000",
            "8192",
            None,
        )
        .expect("build OpenCode settings");

        assert_eq!(cfg["npm"], crate::opencode_config::OPENCODE_DEFAULT_NPM);
        assert_eq!(cfg["options"]["apiKey"], "sk-oc");
        assert_eq!(cfg["options"]["baseURL"], "https://api.example.com/v1");
        assert_eq!(cfg["models"]["gpt-4.1-mini"]["name"], "GPT 4.1 Mini");
        assert_eq!(cfg["models"]["gpt-4.1-mini"]["limit"]["context"], 128000);
        assert_eq!(cfg["models"]["gpt-4.1-mini"]["limit"]["output"], 8192);

        serde_json::from_value::<crate::provider::OpenCodeProviderConfig>(cfg)
            .expect("OpenCode schema should accept generated settings");
    }

    #[test]
    fn build_opencode_settings_config_omits_blank_options_and_models() {
        let cfg =
            build_opencode_settings_config(None, "", "", "", "", "", "not-a-number", "-1", None)
                .expect("build OpenCode settings");
        let obj = cfg.as_object().expect("settings object");

        assert_eq!(
            obj.get("npm"),
            Some(&json!(crate::opencode_config::OPENCODE_DEFAULT_NPM))
        );
        assert!(obj.get("options").is_none());
        assert!(obj.get("models").is_none());
    }

    #[test]
    fn build_opencode_settings_config_preserves_unknown_fields_and_removes_renamed_original() {
        let cfg = build_opencode_settings_config(
            Some(&json!({
                "npm": "@ai-sdk/openai-compatible",
                "options": {
                    "apiKey": "sk-old",
                    "baseURL": "https://old.example/v1",
                    "headers": {
                        "X-Test": "1"
                    },
                    "setCacheKey": true
                },
                "models": {
                    "primary": {
                        "name": "Primary",
                        "limit": {
                            "context": 128000,
                            "output": 8192
                        },
                        "options": {
                            "reasoningEffort": "medium"
                        },
                        "providerHint": "reasoning"
                    },
                    "fallback": {
                        "name": "Fallback",
                        "options": {
                            "fallback": true
                        }
                    }
                },
                "topExtra": 1
            })),
            "@ai-sdk/anthropic",
            "",
            "",
            "renamed",
            "Renamed",
            "256000",
            "",
            Some("primary"),
        )
        .expect("build OpenCode settings");

        assert_eq!(cfg["npm"], "@ai-sdk/anthropic");
        assert_eq!(cfg["topExtra"], 1);
        assert!(cfg["options"].get("apiKey").is_none());
        assert!(cfg["options"].get("baseURL").is_none());
        assert_eq!(cfg["options"]["headers"]["X-Test"], "1");
        assert_eq!(cfg["options"]["setCacheKey"], true);
        assert!(cfg["models"].get("primary").is_none());
        assert_eq!(cfg["models"]["fallback"]["options"]["fallback"], true);
        assert_eq!(cfg["models"]["renamed"]["name"], "Renamed");
        assert_eq!(cfg["models"]["renamed"]["limit"]["context"], 256000);
        assert!(cfg["models"]["renamed"]["limit"].get("output").is_none());
    }

    #[test]
    fn opencode_edit_defaults_match_tui_model_selection() {
        let defaults = OpenCodePromptDefaults::from_settings(Some(&json!({
            "npm": "@ai-sdk/anthropic",
            "options": {
                "apiKey": "sk-existing",
                "baseURL": "https://api.existing.example/v1"
            },
            "models": {
                "beta-model": {
                    "name": "Beta Model",
                    "limit": {
                        "context": 64000
                    }
                },
                "alpha-model": {
                    "name": "Alpha Model",
                    "options": {
                        "reasoningEffort": "medium"
                    },
                    "limit": {
                        "context": 128000,
                        "output": 8192
                    }
                }
            }
        })));

        assert_eq!(defaults.npm, "@ai-sdk/anthropic");
        assert_eq!(defaults.api_key, "sk-existing");
        assert_eq!(defaults.base_url, "https://api.existing.example/v1");
        assert_eq!(defaults.model_id, "alpha-model");
        assert_eq!(defaults.model_name, "Alpha Model");
        assert_eq!(defaults.model_context_limit, "128000");
        assert_eq!(defaults.model_output_limit, "8192");
        assert_eq!(defaults.original_model_id.as_deref(), Some("alpha-model"));
    }

    #[test]
    fn build_openclaw_settings_config_writes_canonical_shape() {
        let cfg = build_openclaw_settings_config(
            None,
            "",
            " sk-openclaw ",
            " https://api.openclaw.example/v1 ",
            true,
            json!([
                {
                    "id": "primary-model",
                    "name": "Primary Model",
                    "contextWindow": 128000
                }
            ]),
        )
        .expect("build OpenClaw settings");

        assert_eq!(
            cfg["api"],
            crate::openclaw_config::OPENCLAW_DEFAULT_API_PROTOCOL
        );
        assert_eq!(cfg["apiKey"], "sk-openclaw");
        assert_eq!(cfg["baseUrl"], "https://api.openclaw.example/v1");
        assert_eq!(
            cfg["headers"]["User-Agent"],
            crate::openclaw_config::OPENCLAW_DEFAULT_USER_AGENT
        );
        assert_eq!(cfg["models"][0]["id"], "primary-model");
    }

    #[test]
    fn build_openclaw_settings_config_removes_legacy_aliases_and_preserves_extra_headers() {
        let cfg = build_openclaw_settings_config(
            Some(&json!({
                "api_key": "legacy-key",
                "base_url": "https://legacy.example/v1",
                "npm": "@legacy/package",
                "options": {
                    "apiKey": "legacy-options-key"
                },
                "headers": {
                    "User-Agent": "Existing UA",
                    "X-Test": "1"
                },
                "authHeader": true,
                "models": [
                    {
                        "id": "old-model"
                    }
                ]
            })),
            "anthropic-messages",
            "",
            "",
            false,
            json!([
                {
                    "id": "new-model",
                    "name": "New Model",
                    "context_window": 128000
                }
            ]),
        )
        .expect("build OpenClaw settings");
        let obj = cfg.as_object().expect("settings object");

        assert_eq!(obj.get("api"), Some(&json!("anthropic-messages")));
        assert_eq!(obj.get("authHeader"), Some(&json!(true)));
        assert_eq!(cfg["headers"]["X-Test"], "1");
        assert!(cfg["headers"].get("User-Agent").is_none());
        assert!(obj.get("apiKey").is_none());
        assert!(obj.get("baseUrl").is_none());
        assert!(obj.get("api_key").is_none());
        assert!(obj.get("base_url").is_none());
        assert!(obj.get("npm").is_none());
        assert!(obj.get("options").is_none());
        assert_eq!(cfg["models"][0]["id"], "new-model");
        assert!(
            cfg["models"][0].get("context_window").is_none(),
            "CLI should remove legacy OpenClaw model aliases before saving"
        );
    }

    #[test]
    fn build_openclaw_settings_config_rejects_non_array_or_empty_models() {
        let non_array_err =
            build_openclaw_settings_config(None, "", "", "", false, json!({"id": "model"}))
                .expect_err("non-array models should fail");
        assert!(non_array_err.to_string().contains("models"));

        let empty_err = build_openclaw_settings_config(None, "", "", "", false, json!([]))
            .expect_err("empty models should fail");
        assert!(empty_err.to_string().contains("models"));
    }

    #[test]
    fn openclaw_cubence_template_prompt_can_preserve_omitted_models() {
        let seed =
            build_provider_template_seed(&AppType::OpenClaw, ProviderAddTemplate::Cubence, &[])
                .expect("build Cubence OpenClaw seed");
        assert!(
            seed.settings_config.get("models").is_none(),
            "Cubence seed should match TUI by omitting models"
        );

        let models = parse_openclaw_models_json("", false)
            .expect("template prompt should allow omitted models");
        let cfg = build_openclaw_settings_config_with_optional_models(
            Some(&seed.settings_config),
            crate::openclaw_config::OPENCLAW_DEFAULT_API_PROTOCOL,
            "",
            seed.settings_config["baseUrl"]
                .as_str()
                .expect("seed baseUrl should exist"),
            false,
            models,
        )
        .expect("build OpenClaw settings");

        assert_eq!(
            cfg["api"],
            crate::openclaw_config::OPENCLAW_DEFAULT_API_PROTOCOL
        );
        assert_eq!(cfg["baseUrl"], "https://api.cubence.com");
        assert!(
            cfg.get("models").is_none(),
            "OpenClaw Cubence template prompt must preserve TUI omitted-models shape"
        );
    }

    #[test]
    fn openclaw_custom_add_still_requires_models() {
        let err = parse_openclaw_models_json("", true)
            .expect_err("custom OpenClaw add should still require models");
        assert!(err.to_string().contains("models"));
    }

    #[test]
    fn openclaw_edit_defaults_read_canonical_settings() {
        let defaults = OpenClawPromptDefaults::from_settings(Some(&json!({
            "api": "openai-responses",
            "apiKey": "sk-existing",
            "baseUrl": "https://api.existing.example/v1",
            "headers": {
                "User-Agent": "Existing UA"
            },
            "models": [
                {
                    "id": "existing-model",
                    "contextWindow": 200000
                }
            ]
        })));

        assert_eq!(defaults.api, "openai-responses");
        assert_eq!(defaults.api_key, "sk-existing");
        assert_eq!(defaults.base_url, "https://api.existing.example/v1");
        assert!(defaults.user_agent_enabled);
        assert!(defaults.models_json.contains("existing-model"));
    }
}

fn build_codex_settings_config(
    api_key: Option<&str>,
    base_url: &str,
    model: &str,
    wire_api: &str,
    provider_name: &str,
) -> Value {
    let model = if model.trim().is_empty() {
        "gpt-5.4"
    } else {
        model.trim()
    };
    let base_url = base_url.trim();

    let config_toml = crate::codex_config::build_codex_third_party_config_toml(
        provider_name,
        base_url,
        model,
        wire_api,
    )
    .trim()
    .to_string();

    match api_key {
        Some(key) if !key.trim().is_empty() => {
            json!({
                "auth": { "OPENAI_API_KEY": key.trim() },
                "config": config_toml
            })
        }
        None => json!({
            "config": config_toml
        }),
        Some(_) => json!({
            "config": config_toml
        }),
    }
}

/// Parse a Codex provider's stored `settings_config` and extract the effective
/// `base_url` and `model` from its `config` TOML, supporting both the flat
/// (`base_url = ...`) and full upstream (`model_providers.<key>.base_url`) shapes.
/// Used by the non-interactive `provider add` to inherit template-provided values
/// when the caller does not pass `--base-url` / `--model`.
pub(crate) fn codex_current_base_url_model(
    current: Option<&Value>,
) -> (Option<String>, Option<String>) {
    let Some(cfg) = current
        .and_then(|v| v.get("config"))
        .and_then(|c| c.as_str())
    else {
        return (None, None);
    };
    let Ok(table) = toml::from_str::<toml::Table>(cfg) else {
        return (None, None);
    };
    let mut base_url = table
        .get("base_url")
        .and_then(|v| v.as_str())
        .map(String::from);
    if base_url.is_none() {
        if let (Some(model_provider), Some(model_providers)) = (
            table.get("model_provider").and_then(|v| v.as_str()),
            table.get("model_providers").and_then(|v| v.as_table()),
        ) {
            base_url = model_providers
                .get(model_provider)
                .and_then(|v| v.as_table())
                .and_then(|t| t.get("base_url"))
                .and_then(|v| v.as_str())
                .map(String::from);
        }
    }
    let model = table
        .get("model")
        .and_then(|v| v.as_str())
        .map(String::from);
    (base_url, model)
}

pub(crate) fn build_codex_settings_config_from_prompt(
    current: Option<&Value>,
    api_key: &str,
    base_url: &str,
    model: &str,
    provider_name: &str,
) -> Value {
    let model = if model.trim().is_empty() {
        "gpt-5.4"
    } else {
        model.trim()
    };
    let base_url = base_url.trim().trim_end_matches('/');
    let current_config = current
        .and_then(|value| value.get("config"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let base_config = if current_config.trim().is_empty() {
        crate::codex_config::build_codex_third_party_config_toml(
            provider_name,
            base_url,
            model,
            "responses",
        )
    } else {
        current_config.to_string()
    };
    let config_toml = crate::codex_config::update_codex_config_snippet(
        &base_config,
        base_url,
        model,
        "responses",
        true,
        "OPENAI_API_KEY",
    );

    let mut settings_obj = current
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    settings_obj.insert("config".to_string(), Value::String(config_toml));

    let api_key = api_key.trim();
    if api_key.is_empty() {
        if let Some(auth_obj) = settings_obj.get_mut("auth").and_then(Value::as_object_mut) {
            auth_obj.remove("OPENAI_API_KEY");
            if auth_obj.is_empty() {
                settings_obj.remove("auth");
            }
        } else {
            settings_obj.remove("auth");
        }
    } else {
        let auth_value = settings_obj
            .entry("auth".to_string())
            .or_insert_with(|| json!({}));
        if !auth_value.is_object() {
            *auth_value = json!({});
        }
        let auth = auth_value
            .as_object_mut()
            .expect("auth must be a JSON object");
        auth.insert("OPENAI_API_KEY".to_string(), json!(api_key));
    }

    Value::Object(settings_obj)
}

pub(crate) fn build_gemini_oauth_settings_config(current: Option<&Value>) -> Value {
    let mut settings_obj = current
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let env_value = settings_obj
        .entry("env".to_string())
        .or_insert_with(|| json!({}));
    if !env_value.is_object() {
        *env_value = json!({});
    }
    let env_obj = env_value
        .as_object_mut()
        .expect("Gemini env settings must be an object");
    env_obj.remove("GEMINI_API_KEY");
    env_obj.remove("GOOGLE_GEMINI_BASE_URL");
    env_obj.remove("GEMINI_BASE_URL");
    env_obj.remove("GEMINI_MODEL");
    Value::Object(settings_obj)
}

pub(crate) fn build_gemini_api_key_settings_config(
    current: Option<&Value>,
    api_key: &str,
    base_url: &str,
    model: &str,
) -> Value {
    let mut settings_obj = current
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let env_value = settings_obj
        .entry("env".to_string())
        .or_insert_with(|| json!({}));
    if !env_value.is_object() {
        *env_value = json!({});
    }
    let env = env_value
        .as_object_mut()
        .expect("Gemini env settings must be an object");
    set_or_remove_trimmed(env, "GEMINI_API_KEY", api_key);
    set_or_remove_trimmed(env, "GOOGLE_GEMINI_BASE_URL", base_url);
    set_or_remove_trimmed(env, "GEMINI_MODEL", model);
    Value::Object(settings_obj)
}

fn build_codex_official_settings_config(current: Option<&Value>) -> Result<Value, AppError> {
    let auth = current
        .and_then(|value| value.get("auth"))
        .and_then(Value::as_object)
        .map(|value| Value::Object(value.clone()))
        .unwrap_or_else(|| json!({}));
    let config = current
        .and_then(|value| value.get("config"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let cleaned_config = crate::codex_config::strip_codex_provider_config_text(config)?;

    Ok(json!({
        "auth": auth,
        "config": cleaned_config
    }))
}

struct OpenCodePromptDefaults {
    npm: String,
    api_key: String,
    base_url: String,
    model_id: String,
    model_name: String,
    model_context_limit: String,
    model_output_limit: String,
    original_model_id: Option<String>,
}

impl OpenCodePromptDefaults {
    fn from_settings(current: Option<&Value>) -> Self {
        let settings = current.and_then(Value::as_object);
        let npm = settings
            .and_then(|obj| obj.get("npm"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(crate::opencode_config::OPENCODE_DEFAULT_NPM)
            .to_string();

        let options = settings
            .and_then(|obj| obj.get("options"))
            .and_then(Value::as_object);
        let api_key = options
            .and_then(|obj| obj.get("apiKey"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let base_url = options
            .and_then(|obj| obj.get("baseURL"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let mut model_id = String::new();
        let mut model_name = String::new();
        let mut model_context_limit = String::new();
        let mut model_output_limit = String::new();
        let mut original_model_id = None;
        if let Some(models) = settings
            .and_then(|obj| obj.get("models"))
            .and_then(Value::as_object)
        {
            if let Some((selected_id, model_value)) = opencode_selected_model_from_models(models) {
                model_id = selected_id.clone();
                model_name = model_value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(selected_id)
                    .to_string();
                if let Some(limit) = model_value.get("limit").and_then(Value::as_object) {
                    if let Some(context) = limit.get("context").and_then(Value::as_u64) {
                        model_context_limit = context.to_string();
                    }
                    if let Some(output) = limit.get("output").and_then(Value::as_u64) {
                        model_output_limit = output.to_string();
                    }
                }
                original_model_id = Some(selected_id.clone());
            }
        }

        Self {
            npm,
            api_key,
            base_url,
            model_id,
            model_name,
            model_context_limit,
            model_output_limit,
            original_model_id,
        }
    }
}

fn prompt_opencode_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::tui_label_app_opencode().bright_cyan().bold());

    let defaults = OpenCodePromptDefaults::from_settings(current);

    let npm = Text::new(texts::tui_label_provider_package())
        .with_initial_value(&defaults.npm)
        .with_help_message(opencode_npm_help())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let api_key = if defaults.api_key.is_empty() {
        Text::new(texts::api_key_label())
            .with_placeholder("sk-...")
            .with_help_message(texts::api_key_help())
            .prompt()
    } else {
        Text::new(texts::api_key_label())
            .with_initial_value(&defaults.api_key)
            .with_help_message(texts::api_key_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let base_url = if defaults.base_url.is_empty() {
        Text::new(texts::base_url_label())
            .with_placeholder("https://api.example.com/v1")
            .with_help_message(opencode_base_url_help())
            .prompt()
    } else {
        Text::new(texts::base_url_label())
            .with_initial_value(&defaults.base_url)
            .with_help_message(opencode_base_url_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let model_id = if defaults.model_id.is_empty() {
        Text::new(texts::tui_label_opencode_model_id())
            .with_placeholder("gpt-4.1-mini")
            .with_help_message(opencode_model_id_help())
            .prompt()
    } else {
        Text::new(texts::tui_label_opencode_model_id())
            .with_initial_value(&defaults.model_id)
            .with_help_message(opencode_model_id_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let model_name = if defaults.model_name.is_empty() {
        Text::new(texts::tui_label_opencode_model_name())
            .with_placeholder("GPT 4.1 Mini")
            .with_help_message(opencode_model_name_help())
            .prompt()
    } else {
        Text::new(texts::tui_label_opencode_model_name())
            .with_initial_value(&defaults.model_name)
            .with_help_message(opencode_model_name_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let model_context_limit = if defaults.model_context_limit.is_empty() {
        Text::new(texts::tui_label_context_limit())
            .with_placeholder("128000")
            .with_help_message(opencode_limit_help())
            .prompt()
    } else {
        Text::new(texts::tui_label_context_limit())
            .with_initial_value(&defaults.model_context_limit)
            .with_help_message(opencode_limit_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let model_output_limit = if defaults.model_output_limit.is_empty() {
        Text::new(texts::tui_label_output_limit())
            .with_placeholder("8192")
            .with_help_message(opencode_limit_help())
            .prompt()
    } else {
        Text::new(texts::tui_label_output_limit())
            .with_initial_value(&defaults.model_output_limit)
            .with_help_message(opencode_limit_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    build_opencode_settings_config(
        current,
        &npm,
        &api_key,
        &base_url,
        &model_id,
        &model_name,
        &model_context_limit,
        &model_output_limit,
        defaults.original_model_id.as_deref(),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "OpenCode settings builder maps flat form fields into nested JSON"
)]
fn build_opencode_settings_config(
    current: Option<&Value>,
    npm: &str,
    api_key: &str,
    base_url: &str,
    model_id: &str,
    model_name: &str,
    model_context_limit: &str,
    model_output_limit: &str,
    original_model_id: Option<&str>,
) -> Result<Value, AppError> {
    let mut settings_obj = current
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let npm = npm.trim();
    settings_obj.insert(
        "npm".to_string(),
        json!(if npm.is_empty() {
            crate::opencode_config::OPENCODE_DEFAULT_NPM
        } else {
            npm
        }),
    );

    let options_value = settings_obj
        .entry("options".to_string())
        .or_insert_with(|| json!({}));
    if !options_value.is_object() {
        *options_value = json!({});
    }
    let options_obj = options_value
        .as_object_mut()
        .expect("options must be a JSON object");
    set_or_remove_trimmed(options_obj, "apiKey", api_key);
    set_or_remove_trimmed(options_obj, "baseURL", base_url);
    if options_obj.is_empty() {
        settings_obj.remove("options");
    }

    let mut models_value = settings_obj
        .remove("models")
        .unwrap_or_else(|| Value::Object(Map::new()));
    if !models_value.is_object() {
        models_value = Value::Object(Map::new());
    }
    let models_obj = models_value
        .as_object_mut()
        .expect("models must be a JSON object");

    let current_model_id = opencode_primary_model_id(model_id, model_name);
    if let Some(original_id) = original_model_id {
        if current_model_id.as_deref() != Some(original_id) {
            models_obj.remove(original_id);
        }
    }

    if let Some(model_id) = current_model_id {
        let mut model_obj = match models_obj.remove(&model_id) {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        };
        let model_name = model_name.trim();
        model_obj.insert(
            "name".to_string(),
            json!(if model_name.is_empty() {
                model_id.as_str()
            } else {
                model_name
            }),
        );

        let limit_value = model_obj
            .entry("limit".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !limit_value.is_object() {
            *limit_value = Value::Object(Map::new());
        }
        let limit_obj = limit_value
            .as_object_mut()
            .expect("limit must be a JSON object");
        set_or_remove_u64(limit_obj, "context", model_context_limit);
        set_or_remove_u64(limit_obj, "output", model_output_limit);
        if limit_obj.is_empty() {
            model_obj.remove("limit");
        }

        models_obj.insert(model_id, Value::Object(model_obj));
    }

    if !models_obj.is_empty() {
        settings_obj.insert("models".to_string(), models_value);
    }

    Ok(Value::Object(settings_obj))
}

fn opencode_primary_model_id(model_id: &str, model_name: &str) -> Option<String> {
    let model_id = model_id.trim();
    if !model_id.is_empty() {
        return Some(model_id.to_string());
    }

    let model_name = model_name.trim();
    if !model_name.is_empty() {
        return Some(model_name.to_string());
    }

    None
}

fn opencode_selected_model_from_models(models: &Map<String, Value>) -> Option<(&String, &Value)> {
    models.iter().max_by(|(id_a, model_a), (id_b, model_b)| {
        opencode_model_rank(model_a)
            .cmp(&opencode_model_rank(model_b))
            .then_with(|| id_b.cmp(id_a))
    })
}

fn opencode_model_rank(model: &Value) -> usize {
    let mut rank = 0;
    if model
        .get("limit")
        .and_then(Value::as_object)
        .is_some_and(|obj| !obj.is_empty())
    {
        rank += 1;
    }
    if model
        .get("options")
        .and_then(Value::as_object)
        .is_some_and(|obj| !obj.is_empty())
    {
        rank += 1;
    }
    rank
}

fn opencode_npm_help() -> &'static str {
    if crate::cli::i18n::is_chinese() {
        "AI SDK provider npm 包；留空使用 OpenAI-compatible 默认值。"
    } else {
        "AI SDK provider npm package; leave empty for the OpenAI-compatible default."
    }
}

fn opencode_base_url_help() -> &'static str {
    if crate::cli::i18n::is_chinese() {
        "OpenCode provider options.baseURL；留空则不写入。"
    } else {
        "OpenCode provider options.baseURL; leave empty to omit it."
    }
}

fn opencode_model_id_help() -> &'static str {
    if crate::cli::i18n::is_chinese() {
        "models 对象中的主模型键；留空时使用模型名称。"
    } else {
        "Primary model key inside models; the model name is used when this is empty."
    }
}

fn opencode_model_name_help() -> &'static str {
    if crate::cli::i18n::is_chinese() {
        "OpenCode 模型显示名称；留空时使用模型 ID。"
    } else {
        "OpenCode model display name; the model id is used when this is empty."
    }
}

fn opencode_limit_help() -> &'static str {
    if crate::cli::i18n::is_chinese() {
        "正整数；留空或无效值会移除此限制。"
    } else {
        "Positive integer; empty or invalid values remove this limit."
    }
}

struct HermesPromptDefaults {
    api_mode: String,
    api_key: String,
    base_url: String,
    models_json: String,
    rate_limit_delay: String,
}

impl HermesPromptDefaults {
    fn from_settings(current: Option<&Value>) -> Self {
        let settings = current.and_then(Value::as_object);
        let api_mode = settings
            .and_then(|obj| obj.get("api_mode").or_else(|| obj.get("apiMode")))
            .and_then(Value::as_str)
            .map(normalize_hermes_api_mode)
            .unwrap_or_else(|| crate::hermes_config::HERMES_DEFAULT_API_MODE.to_string());
        let api_key = settings
            .and_then(|obj| {
                obj.get("api_key")
                    .or_else(|| obj.get("apiKey"))
                    .or_else(|| obj.get("auth_token"))
            })
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let base_url = settings
            .and_then(|obj| {
                obj.get("base_url")
                    .or_else(|| obj.get("baseUrl"))
                    .or_else(|| obj.get("baseURL"))
                    .or_else(|| obj.get("endpoint"))
            })
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let models_json = settings
            .and_then(|obj| obj.get("models"))
            .and_then(Value::as_array)
            .map(|models| Value::Array(models.clone()))
            .and_then(|value| serde_json::to_string(&value).ok())
            .unwrap_or_else(|| "[]".to_string());
        let rate_limit_delay = settings
            .and_then(|obj| obj.get("rate_limit_delay"))
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| value.to_string())
            .unwrap_or_default();

        Self {
            api_mode,
            api_key,
            base_url,
            models_json,
            rate_limit_delay,
        }
    }
}

fn prompt_hermes_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::tui_label_app_hermes().bright_cyan().bold());

    let defaults = HermesPromptDefaults::from_settings(current);
    let api_modes = crate::hermes_config::HERMES_API_MODES
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    let api_mode_index = api_modes
        .iter()
        .position(|candidate| candidate == &defaults.api_mode)
        .unwrap_or(0);

    let api_mode = Select::new(texts::tui_label_hermes_api_mode(), api_modes)
        .with_starting_cursor(api_mode_index)
        .with_help_message(hermes_api_mode_help())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let base_url = if defaults.base_url.is_empty() {
        Text::new(texts::tui_label_hermes_base_url())
            .with_placeholder("https://api.example.com/v1")
            .with_help_message(texts::tui_hermes_base_url_scheme())
            .prompt()
    } else {
        Text::new(texts::tui_label_hermes_base_url())
            .with_initial_value(&defaults.base_url)
            .with_help_message(texts::tui_hermes_base_url_scheme())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let api_key = if defaults.api_key.is_empty() {
        Text::new(texts::api_key_label())
            .with_placeholder("sk-...")
            .with_help_message(texts::api_key_help())
            .prompt()
    } else {
        Text::new(texts::api_key_label())
            .with_initial_value(&defaults.api_key)
            .with_help_message(texts::api_key_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let models_json = if defaults.models_json == "[]" {
        Text::new(texts::tui_label_hermes_models())
            .with_placeholder(r#"[{"id":"gpt-4.1","name":"GPT 4.1"}]"#)
            .with_help_message(hermes_models_json_help())
            .prompt()
    } else {
        Text::new(texts::tui_label_hermes_models())
            .with_initial_value(&defaults.models_json)
            .with_help_message(hermes_models_json_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;
    let models_value = parse_hermes_models_json(&models_json)?;

    let rate_limit_delay = if defaults.rate_limit_delay.is_empty() {
        Text::new(texts::tui_label_hermes_rate_limit_delay())
            .with_placeholder("0.5")
            .with_help_message(texts::tui_hint_hermes_rate_limit_delay())
            .prompt()
    } else {
        Text::new(texts::tui_label_hermes_rate_limit_delay())
            .with_initial_value(&defaults.rate_limit_delay)
            .with_help_message(texts::tui_hint_hermes_rate_limit_delay())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    build_hermes_settings_config(
        current,
        &api_mode,
        &base_url,
        &api_key,
        models_value,
        &rate_limit_delay,
    )
}

fn build_hermes_settings_config(
    current: Option<&Value>,
    api_mode: &str,
    base_url: &str,
    api_key: &str,
    models_value: Value,
    rate_limit_delay: &str,
) -> Result<Value, AppError> {
    let mut settings_obj = current
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for legacy_key in ["api", "apiKey", "apiMode", "baseUrl", "baseURL", "endpoint"] {
        settings_obj.remove(legacy_key);
    }

    settings_obj.insert(
        "api_mode".to_string(),
        json!(normalize_hermes_api_mode(api_mode)),
    );

    let base_url = base_url.trim().trim_end_matches('/').to_string();
    set_or_remove_trimmed(&mut settings_obj, "base_url", &base_url);
    set_or_remove_trimmed(&mut settings_obj, "api_key", api_key);

    let models_value = normalize_hermes_models_value(models_value)?;
    if models_value.as_array().is_some_and(Vec::is_empty) {
        settings_obj.remove("models");
    } else {
        settings_obj.insert("models".to_string(), models_value);
    }

    set_or_remove_f64(&mut settings_obj, "rate_limit_delay", rate_limit_delay);

    Ok(Value::Object(settings_obj))
}

fn normalize_hermes_api_mode(api_mode: &str) -> String {
    let api_mode = api_mode.trim();
    if crate::hermes_config::HERMES_API_MODES.contains(&api_mode) {
        api_mode.to_string()
    } else {
        crate::hermes_config::HERMES_DEFAULT_API_MODE.to_string()
    }
}

fn parse_hermes_models_json(raw: &str) -> Result<Value, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(json!([]));
    }
    let value = serde_json::from_str::<Value>(trimmed)
        .map_err(|err| AppError::InvalidInput(texts::tui_toast_invalid_json(&err.to_string())))?;
    normalize_hermes_models_value(value)
}

fn normalize_hermes_models_value(value: Value) -> Result<Value, AppError> {
    if value.is_array() {
        Ok(value)
    } else {
        Err(AppError::localized(
            "provider.hermes.models.invalid",
            "Hermes 模型列表必须是 JSON 数组",
            "Hermes models must be a JSON array",
        ))
    }
}

fn hermes_api_mode_help() -> &'static str {
    if crate::cli::i18n::is_chinese() {
        "供应商 API 协议。请选择与端点匹配的格式。"
    } else {
        "Provider API protocol. Choose the format that matches your endpoint."
    }
}

fn hermes_models_json_help() -> &'static str {
    if crate::cli::i18n::is_chinese() {
        "JSON 数组；留空或 [] 表示不写入模型。"
    } else {
        "JSON array; leave empty or [] to omit models."
    }
}

struct OpenClawPromptDefaults {
    api: String,
    api_key: String,
    base_url: String,
    user_agent_enabled: bool,
    models_json: String,
}

impl OpenClawPromptDefaults {
    fn from_settings(current: Option<&Value>) -> Self {
        let settings = current.and_then(Value::as_object);
        let api = settings
            .and_then(|obj| obj.get("api"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(crate::openclaw_config::OPENCLAW_DEFAULT_API_PROTOCOL)
            .to_string();
        let api_key = settings
            .and_then(|obj| obj.get("apiKey"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let base_url = settings
            .and_then(|obj| obj.get("baseUrl"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let user_agent_enabled = settings
            .and_then(|obj| obj.get("headers"))
            .and_then(Value::as_object)
            .is_some_and(|headers| headers.contains_key("User-Agent"));
        let models_json = settings
            .and_then(|obj| obj.get("models"))
            .and_then(Value::as_array)
            .map(|models| Value::Array(models.clone()))
            .and_then(|value| serde_json::to_string(&value).ok())
            .unwrap_or_else(|| "[]".to_string());
        Self {
            api,
            api_key,
            base_url,
            user_agent_enabled,
            models_json,
        }
    }
}

fn prompt_openclaw_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::config_openclaw_header().bright_cyan().bold());

    let defaults = OpenClawPromptDefaults::from_settings(current);
    let mut api_protocols = crate::openclaw_config::OPENCLAW_API_PROTOCOLS
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    if !api_protocols
        .iter()
        .any(|candidate| candidate == &defaults.api)
    {
        api_protocols.insert(0, defaults.api.clone());
    }
    let api_index = api_protocols
        .iter()
        .position(|candidate| candidate == &defaults.api)
        .unwrap_or(0);

    let api = Select::new(texts::openclaw_api_protocol_label(), api_protocols)
        .with_starting_cursor(api_index)
        .with_help_message(texts::openclaw_api_protocol_help())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let api_key = if defaults.api_key.is_empty() {
        Text::new(texts::api_key_label())
            .with_placeholder("sk-...")
            .with_help_message(texts::api_key_help())
            .prompt()
    } else {
        Text::new(texts::api_key_label())
            .with_initial_value(&defaults.api_key)
            .with_help_message(texts::api_key_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let base_url = if defaults.base_url.is_empty() {
        Text::new(texts::base_url_label())
            .with_placeholder("https://api.example.com/v1")
            .with_help_message(texts::openclaw_base_url_help())
            .prompt()
    } else {
        Text::new(texts::base_url_label())
            .with_initial_value(&defaults.base_url)
            .with_help_message(texts::openclaw_base_url_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let user_agent_enabled = Confirm::new(texts::openclaw_user_agent_prompt())
        .with_default(defaults.user_agent_enabled)
        .with_help_message(texts::openclaw_user_agent_help())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let models_json = if defaults.models_json == "[]" {
        Text::new(texts::openclaw_models_json_label())
            .with_placeholder(r#"[{"id":"gpt-4.1","name":"GPT 4.1"}]"#)
            .with_help_message(texts::openclaw_models_json_help())
            .prompt()
    } else {
        Text::new(texts::openclaw_models_json_label())
            .with_initial_value(&defaults.models_json)
            .with_help_message(texts::openclaw_models_json_help())
            .prompt()
    }
    .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;
    let models_value = parse_openclaw_models_json(&models_json, current.is_none())?;

    build_openclaw_settings_config_with_optional_models(
        current,
        &api,
        &api_key,
        &base_url,
        user_agent_enabled,
        models_value,
    )
}

fn build_openclaw_settings_config(
    current: Option<&Value>,
    api: &str,
    api_key: &str,
    base_url: &str,
    user_agent_enabled: bool,
    models_value: Value,
) -> Result<Value, AppError> {
    build_openclaw_settings_config_with_optional_models(
        current,
        api,
        api_key,
        base_url,
        user_agent_enabled,
        Some(models_value),
    )
}

fn build_openclaw_settings_config_with_optional_models(
    current: Option<&Value>,
    api: &str,
    api_key: &str,
    base_url: &str,
    user_agent_enabled: bool,
    models_value: Option<Value>,
) -> Result<Value, AppError> {
    let mut settings_obj = current
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for legacy_key in ["npm", "options", "api_key", "base_url"] {
        settings_obj.remove(legacy_key);
    }

    set_or_remove_trimmed(&mut settings_obj, "apiKey", api_key);
    set_or_remove_trimmed(&mut settings_obj, "baseUrl", base_url);

    let api = api.trim();
    settings_obj.insert(
        "api".to_string(),
        json!(if api.is_empty() {
            crate::openclaw_config::OPENCLAW_DEFAULT_API_PROTOCOL
        } else {
            api
        }),
    );

    let mut headers_obj = match settings_obj.remove("headers") {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    if user_agent_enabled {
        headers_obj
            .entry("User-Agent".to_string())
            .or_insert_with(|| json!(crate::openclaw_config::OPENCLAW_DEFAULT_USER_AGENT));
    } else {
        headers_obj.remove("User-Agent");
    }
    if !headers_obj.is_empty() {
        settings_obj.insert("headers".to_string(), Value::Object(headers_obj));
    }

    if let Some(models_value) = models_value {
        let models_value = normalize_openclaw_models_value(models_value)?;
        settings_obj.insert("models".to_string(), models_value);
    } else {
        settings_obj.remove("models");
    }

    serde_json::from_value::<crate::provider::OpenClawProviderConfig>(Value::Object(
        settings_obj.clone(),
    ))
    .map_err(|err| {
        AppError::localized(
            "provider.openclaw.settings.invalid",
            format!("OpenClaw 配置格式无效: {err}"),
            format!("OpenClaw provider schema is invalid: {err}"),
        )
    })?;

    Ok(Value::Object(settings_obj))
}

fn parse_openclaw_models_json(
    raw: &str,
    require_non_empty: bool,
) -> Result<Option<Value>, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return if require_non_empty {
            Err(openclaw_models_required_error())
        } else {
            Ok(None)
        };
    }
    let value = serde_json::from_str::<Value>(trimmed)
        .map_err(|err| AppError::InvalidInput(texts::tui_toast_invalid_json(&err.to_string())))?;
    if !require_non_empty && value.as_array().is_some_and(Vec::is_empty) {
        return Ok(None);
    }
    normalize_openclaw_models_value(value).map(Some)
}

fn normalize_openclaw_models_value(value: Value) -> Result<Value, AppError> {
    let Some(models) = value.as_array() else {
        return Err(openclaw_models_required_error());
    };
    if models.is_empty() {
        return Err(openclaw_models_required_error());
    }

    let normalized_models = models
        .iter()
        .cloned()
        .map(remove_openclaw_model_legacy_aliases)
        .collect::<Vec<_>>();
    let normalized_value = Value::Array(normalized_models);

    serde_json::from_value::<Vec<crate::provider::OpenClawModelEntry>>(normalized_value.clone())
        .map_err(|err| {
            AppError::InvalidInput(texts::openclaw_models_invalid_schema_error(
                &err.to_string(),
            ))
        })?;

    Ok(normalized_value)
}

fn remove_openclaw_model_legacy_aliases(model: Value) -> Value {
    let Value::Object(mut model_obj) = model else {
        return model;
    };
    model_obj.remove("context_window");
    Value::Object(model_obj)
}

fn openclaw_models_required_error() -> AppError {
    AppError::localized(
        "provider.openclaw.models.missing",
        "OpenClaw 模型列表必须是非空 JSON 数组",
        "OpenClaw models must be a non-empty JSON array",
    )
}

fn set_or_remove_trimmed(settings_obj: &mut Map<String, Value>, key: &str, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        settings_obj.remove(key);
    } else {
        settings_obj.insert(key.to_string(), json!(trimmed));
    }
}

fn set_or_remove_u64(settings_obj: &mut Map<String, Value>, key: &str, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        settings_obj.remove(key);
    } else if let Ok(value) = trimmed.parse::<u64>() {
        settings_obj.insert(key.to_string(), json!(value));
    } else {
        settings_obj.remove(key);
    }
}

fn set_or_remove_f64(settings_obj: &mut Map<String, Value>, key: &str, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        settings_obj.remove(key);
    } else if let Ok(value) = trimmed.parse::<f64>() {
        if value.is_finite() && value >= 0.0 {
            settings_obj.insert(key.to_string(), json!(value));
        } else {
            settings_obj.remove(key);
        }
    } else {
        settings_obj.remove(key);
    }
}

/// 可选字段集合
#[derive(Default)]
pub struct OptionalFields {
    pub notes: Option<String>,
    pub icon: Option<String>,
    pub icon_color: Option<String>,
    pub sort_index: Option<usize>,
}

impl OptionalFields {
    /// 从现有 Provider 提取可选字段
    pub fn from_provider(provider: &Provider) -> Self {
        Self {
            notes: provider.notes.clone(),
            icon: provider.icon.clone(),
            icon_color: provider.icon_color.clone(),
            sort_index: provider.sort_index,
        }
    }
}

/// 生成唯一的 Provider ID
/// 基于名称转换为 kebab-case，如有冲突则追加数字后缀
pub fn generate_provider_id(name: &str, existing_ids: &[String]) -> String {
    // 转换为 kebab-case
    let base_id = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    // 检查唯一性
    if !existing_ids.contains(&base_id) {
        return base_id;
    }

    // 追加数字后缀
    let mut counter = 1;
    loop {
        let candidate = format!("{}-{}", base_id, counter);
        if !existing_ids.contains(&candidate) {
            return candidate;
        }
        counter += 1;
    }
}

pub fn generate_provider_id_for_app(
    app_type: &AppType,
    name: &str,
    existing_ids: &[String],
) -> String {
    if ProviderService::is_provider_key_app(app_type) {
        ProviderService::generate_provider_key(name, existing_ids)
    } else {
        generate_provider_id(name, existing_ids)
    }
}

pub fn validate_provider_id_for_add(
    app_type: &AppType,
    id: &str,
    existing_ids: &[String],
) -> Result<(), AppError> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput(
            texts::provider_id_empty_error().to_string(),
        ));
    }
    ProviderService::validate_provider_key_for_add(app_type, trimmed)?;
    if existing_ids.iter().any(|existing| existing == trimmed) {
        return Err(AppError::localized(
            "provider.id.exists",
            format!("供应商 ID 已存在: {trimmed}"),
            format!("Provider ID already exists: {trimmed}"),
        ));
    }
    Ok(())
}

/// 收集基本字段：name, website_url
pub fn prompt_basic_fields(
    current: Option<&Provider>,
) -> Result<(String, Option<String>), AppError> {
    // 供应商名称：根据上下文选择方法
    let name = if let Some(provider) = current {
        // 编辑模式：预填充当前值
        Text::new(texts::provider_name_label())
            .with_initial_value(&provider.name)
            .with_help_message(texts::provider_name_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        // 新增模式：显示示例占位符
        Text::new(texts::provider_name_label())
            .with_placeholder("OpenAI")
            .with_help_message(texts::provider_name_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::InvalidInput(
            texts::provider_name_empty_error().to_string(),
        ));
    }

    // 官网 URL：同样处理
    let website_url = if let Some(provider) = current {
        let initial = provider.website_url.as_deref().unwrap_or("");
        Text::new(texts::website_url_label())
            .with_initial_value(initial)
            .with_help_message(texts::website_url_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::website_url_label())
            .with_placeholder("https://openai.com")
            .with_help_message(texts::website_url_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    let website_url = if website_url.trim().is_empty() {
        None
    } else {
        Some(website_url.trim().to_string())
    };

    Ok((name, website_url))
}

/// 根据应用类型收集 settings_config
pub fn prompt_settings_config(
    app_type: &AppType,
    current: Option<&Value>,
    current_meta: Option<&ProviderMeta>,
    codex_official: bool,
    provider_name: &str,
) -> Result<SettingsConfigPromptResult, AppError> {
    match app_type {
        AppType::Claude => prompt_claude_config(current, current_meta),
        AppType::Codex => {
            if codex_official {
                return prompt_codex_official_config(current).map(SettingsConfigPromptResult::new);
            }

            let has_auth = current
                .and_then(|v| v.get("auth"))
                .and_then(|v| v.as_object())
                .map(|obj| !obj.is_empty())
                .unwrap_or(false);
            let current_config_str = current
                .and_then(|v| v.get("config"))
                .and_then(|c| c.as_str());
            let mut current_base_url: Option<String> = None;
            if let Some(cfg) = current_config_str {
                if let Ok(table) = toml::from_str::<toml::Table>(cfg) {
                    current_base_url = table
                        .get("base_url")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    if current_base_url.is_none() {
                        if let (Some(model_provider), Some(model_providers)) = (
                            table.get("model_provider").and_then(|v| v.as_str()),
                            table.get("model_providers").and_then(|v| v.as_table()),
                        ) {
                            current_base_url = model_providers
                                .get(model_provider)
                                .and_then(|v| v.as_table())
                                .and_then(|t| t.get("base_url"))
                                .and_then(|v| v.as_str())
                                .map(String::from);
                        }
                    }
                }
            }

            let is_openai_official_endpoint = current_base_url
                .as_deref()
                .map(|url| url.trim_start().starts_with("https://api.openai.com"))
                .unwrap_or(false);

            if !has_auth && is_openai_official_endpoint {
                prompt_codex_official_config(current).map(SettingsConfigPromptResult::new)
            } else {
                prompt_codex_config(current, provider_name).map(SettingsConfigPromptResult::new)
            }
        }
        AppType::Gemini => prompt_gemini_config(current).map(SettingsConfigPromptResult::new),
        AppType::OpenCode => prompt_opencode_config(current).map(SettingsConfigPromptResult::new),
        AppType::Hermes => prompt_hermes_config(current).map(SettingsConfigPromptResult::new),
        AppType::OpenClaw => prompt_openclaw_config(current).map(SettingsConfigPromptResult::new),
    }
}

/// 提示用户输入单个模型字段
///
/// # 参数
/// - `field_name`: 字段显示名称（如 "默认模型"）
/// - `env_key`: 环境变量键名（如 "ANTHROPIC_MODEL"）
/// - `placeholder`: 占位符示例值
/// - `current`: 当前配置（编辑模式）
///
/// # 返回
/// - `Some(value)`: 用户输入了值或需要保留现有值
/// - `None`: 用户留空且无现有值，不应写入配置
fn prompt_model_field(
    field_name: &str,
    env_key: &str,
    placeholder: &str,
    current: Option<&Value>,
) -> Result<Option<String>, AppError> {
    // 尝试提取现有值
    let existing_value = current
        .and_then(|v| v.get("env"))
        .and_then(|e| e.get(env_key))
        .and_then(|m| m.as_str());

    let input = if let Some(existing) = existing_value {
        // 编辑模式 - 有现有值：预填充
        Text::new(&format!("{}：", field_name))
            .with_initial_value(existing)
            .with_help_message(texts::model_default_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        // 新增模式或编辑模式无现有值：占位符
        Text::new(&format!("{}：", field_name))
            .with_placeholder(placeholder)
            .with_help_message(texts::model_default_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    let trimmed = input.trim();

    if trimmed.is_empty() {
        if existing_value.is_some() {
            // 编辑模式下清空 → 移除配置
            Ok(None)
        } else {
            // 新增模式或原本无值 → 不写入
            Ok(None)
        }
    } else {
        // 有输入值
        Ok(Some(trimmed.to_string()))
    }
}

fn claude_api_key_field_label(field: ClaudeApiKeyField) -> &'static str {
    match field {
        ClaudeApiKeyField::AuthToken => texts::claude_auth_field_auth_token(),
        ClaudeApiKeyField::ApiKey => texts::claude_auth_field_api_key(),
    }
}

fn prompt_claude_api_key_field(
    current: Option<&Value>,
    current_meta: Option<&ProviderMeta>,
) -> Result<ClaudeApiKeyField, AppError> {
    let current = current.unwrap_or(&Value::Null);
    let effective = ClaudeApiKeyField::from_meta_and_settings(current_meta, current);
    let fields = [ClaudeApiKeyField::AuthToken, ClaudeApiKeyField::ApiKey];
    let choices = fields
        .iter()
        .map(|field| claude_api_key_field_label(*field).to_string())
        .collect::<Vec<_>>();
    let default_index = fields
        .iter()
        .position(|field| *field == effective)
        .unwrap_or(0);

    let selected = Select::new(texts::claude_auth_field_label(), choices.clone())
        .with_starting_cursor(default_index)
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;
    let selected_index = choices
        .iter()
        .position(|choice| choice == &selected)
        .unwrap_or(default_index);
    Ok(fields
        .get(selected_index)
        .copied()
        .unwrap_or(ClaudeApiKeyField::AuthToken))
}

fn claude_api_key_from_settings(
    current: Option<&Value>,
    api_key_field: ClaudeApiKeyField,
) -> Option<&str> {
    let env = current
        .and_then(|v| v.get("env"))
        .and_then(|env| env.as_object())?;

    env.get(api_key_field.as_env_key())
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            env.get(api_key_field.alternate_env_key())
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
        })
}

/// Claude 配置输入
fn prompt_claude_config(
    current: Option<&Value>,
    current_meta: Option<&ProviderMeta>,
) -> Result<SettingsConfigPromptResult, AppError> {
    println!("\n{}", texts::config_claude_header().bright_cyan().bold());

    let api_key_field = prompt_claude_api_key_field(current, current_meta)?;
    let api_key = if let Some(current_key) = claude_api_key_from_settings(current, api_key_field) {
        // 编辑模式：显示完整 API Key 供编辑
        Text::new(texts::api_key_label())
            .with_initial_value(current_key)
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        // 新增模式：占位符示例
        Text::new(texts::api_key_label())
            .with_placeholder("sk-ant-...")
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    let base_url = if let Some(current_url) = current
        .and_then(|v| v.get("env"))
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|u| u.as_str())
        .filter(|s| !s.is_empty())
    {
        Text::new(texts::base_url_label())
            .with_initial_value(current_url)
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::base_url_label())
            .with_placeholder(texts::base_url_placeholder())
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    // 询问是否配置模型
    let config_models = Confirm::new(texts::configure_model_names_prompt())
        .with_default(false)
        .with_help_message(texts::api_key_help())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let mut model_fields = Vec::new();
    if config_models {
        for field in claude_model_prompt_fields() {
            let value = prompt_model_field(field.label, field.env_key, field.placeholder, current)?;
            model_fields.push((field.env_key, value));
        }
    }

    let hide_attribution = Confirm::new(texts::tui_label_claude_hide_attribution())
        .with_default(claude_hide_attribution_enabled(current))
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    Ok(SettingsConfigPromptResult::claude(
        build_claude_settings_config_from_prompt(
            current,
            api_key_field,
            &api_key,
            &base_url,
            model_fields,
            hide_attribution,
        ),
        api_key_field,
    ))
}

#[derive(Debug, Clone, Copy)]
struct ClaudeModelPromptField {
    label: &'static str,
    env_key: &'static str,
    placeholder: &'static str,
}

fn claude_model_prompt_fields() -> [ClaudeModelPromptField; 4] {
    [
        ClaudeModelPromptField {
            label: texts::model_default_label(),
            env_key: "ANTHROPIC_MODEL",
            placeholder: texts::model_sonnet_placeholder(),
        },
        ClaudeModelPromptField {
            label: texts::model_haiku_label(),
            env_key: "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            placeholder: texts::model_haiku_placeholder(),
        },
        ClaudeModelPromptField {
            label: texts::model_sonnet_label(),
            env_key: "ANTHROPIC_DEFAULT_SONNET_MODEL",
            placeholder: texts::model_sonnet_placeholder(),
        },
        ClaudeModelPromptField {
            label: texts::model_opus_label(),
            env_key: "ANTHROPIC_DEFAULT_OPUS_MODEL",
            placeholder: texts::model_opus_placeholder(),
        },
    ]
}

fn claude_hide_attribution_enabled(settings_config: Option<&Value>) -> bool {
    let Some(attribution) = settings_config
        .and_then(|settings| settings.get("attribution"))
        .and_then(Value::as_object)
    else {
        return false;
    };

    attribution.get("commit").and_then(Value::as_str) == Some("")
        && attribution.get("pr").and_then(Value::as_str) == Some("")
}

pub(crate) fn build_claude_settings_config_from_prompt<'a>(
    current: Option<&Value>,
    api_key_field: ClaudeApiKeyField,
    api_key: &str,
    base_url: &str,
    model_fields: impl IntoIterator<Item = (&'a str, Option<String>)>,
    hide_attribution: bool,
) -> Value {
    let mut settings = current
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(Map::new);
    let env_value = settings
        .entry("env".to_string())
        .or_insert_with(|| json!({}));
    if !env_value.is_object() {
        *env_value = json!({});
    }
    let env = env_value
        .as_object_mut()
        .expect("Claude env settings must be an object");

    set_or_remove_trimmed(env, api_key_field.as_env_key(), api_key);
    env.remove(api_key_field.alternate_env_key());
    set_or_remove_trimmed(env, "ANTHROPIC_BASE_URL", base_url);

    for (key, value) in model_fields {
        match value {
            Some(value) => set_or_remove_trimmed(env, key, &value),
            None => {
                env.remove(key);
            }
        }
    }

    let hidden_attribution_already_enabled =
        claude_hide_attribution_enabled(Some(&Value::Object(settings.clone())));
    if hide_attribution && !hidden_attribution_already_enabled {
        settings.insert(
            "attribution".to_string(),
            json!({
                "commit": "",
                "pr": ""
            }),
        );
    } else if !hide_attribution && hidden_attribution_already_enabled {
        settings.remove("attribution");
    }

    Value::Object(settings)
}

/// Codex 配置输入（第三方/自定义：需要 API Key）
fn prompt_codex_config(current: Option<&Value>, provider_name: &str) -> Result<Value, AppError> {
    println!("\n{}", texts::config_codex_header().bright_cyan().bold());

    // 从当前配置提取值
    let current_api_key = current
        .and_then(|v| v.get("auth"))
        .and_then(|a| a.get("OPENAI_API_KEY"))
        .and_then(|k| k.as_str())
        .filter(|s| !s.is_empty());

    let current_config_str = current
        .and_then(|v| v.get("config"))
        .and_then(|c| c.as_str());

    let mut current_base_url: Option<String> = None;
    let mut current_model: Option<String> = None;
    if let Some(cfg) = current_config_str {
        if let Ok(table) = toml::from_str::<toml::Table>(cfg) {
            current_base_url = table
                .get("base_url")
                .and_then(|v| v.as_str())
                .map(String::from);
            if current_base_url.is_none() {
                // Full upstream-style config: base_url lives under model_providers.<model_provider>.
                if let (Some(model_provider), Some(model_providers)) = (
                    table.get("model_provider").and_then(|v| v.as_str()),
                    table.get("model_providers").and_then(|v| v.as_table()),
                ) {
                    current_base_url = model_providers
                        .get(model_provider)
                        .and_then(|v| v.as_table())
                        .and_then(|t| t.get("base_url"))
                        .and_then(|v| v.as_str())
                        .map(String::from);
                }
            }
            current_model = table
                .get("model")
                .and_then(|v| v.as_str())
                .map(String::from);
        }
    }

    // 1. API Key（恢复：用于旧版本 Codex 兼容性）
    let api_key = if let Some(current_key) = current_api_key {
        Text::new(texts::openai_api_key_label())
            .with_initial_value(current_key)
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::openai_api_key_label())
            .with_placeholder("sk-...")
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    // 2. Base URL
    let base_url = if let Some(current) = current_base_url.as_deref() {
        Text::new(&format!("{}:", texts::tui_label_base_url()))
            .with_initial_value(current)
            .with_help_message("API endpoint (e.g., https://api.openai.com/v1)")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(&format!("{}:", texts::tui_label_base_url()))
            .with_placeholder("https://api.openai.com/v1")
            .with_help_message("API endpoint")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };
    let base_url = base_url.trim().to_string();
    if base_url.is_empty() {
        return Err(AppError::InvalidInput(
            texts::base_url_empty_error().to_string(),
        ));
    }

    // 3. Model
    let model = if let Some(current) = current_model.as_deref() {
        Text::new(&format!("{}:", texts::model_label()))
            .with_initial_value(current)
            .with_help_message("Model name (e.g., gpt-5.4, o3)")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(&format!("{}:", texts::model_label()))
            .with_placeholder("gpt-5.4")
            .with_help_message("Model name")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    Ok(build_codex_settings_config_from_prompt(
        current,
        &api_key,
        &base_url,
        model.trim(),
        provider_name,
    ))
}

/// Codex 配置输入（官方：仍写入 provider snapshot 的 auth/config）
fn prompt_codex_official_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::config_codex_header().bright_cyan().bold());
    println!(
        "{}",
        info("OpenAI Official keeps the stored auth snapshot and uses the upstream empty official config.")
    );
    build_codex_official_settings_config(current)
}

/// Gemini 配置输入（含认证类型选择）
fn prompt_gemini_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::config_gemini_header().bright_cyan().bold());

    // 检测当前认证类型
    let current_auth_type = detect_gemini_auth_type(current);
    let default_index = match current_auth_type.as_deref() {
        Some("oauth") => 0,
        _ => 1, // 默认 Generic API Key（包括 packycode 和 generic）
    };

    let auth_options = vec![texts::google_oauth_official(), texts::generic_api_key()];

    let auth_type = Select::new(texts::auth_type_label(), auth_options.clone())
        .with_starting_cursor(default_index)
        .with_help_message(texts::select_auth_method_help())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    // Match using the translated strings
    let google_oauth = texts::google_oauth_official();

    if auth_type == google_oauth {
        println!("{}", texts::use_google_oauth_warning().yellow());
        Ok(build_gemini_oauth_settings_config(current))
    } else {
        // Generic API Key (统一处理所有 API Key 供应商，包括 PackyCode)
        let api_key = if let Some(current_key) = current
            .and_then(|v| v.get("env"))
            .and_then(|e| e.get("GEMINI_API_KEY"))
            .and_then(|k| k.as_str())
            .filter(|s| !s.is_empty())
        {
            Text::new(texts::gemini_api_key_label())
                .with_initial_value(current_key)
                .with_help_message(texts::generic_api_key_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        } else {
            Text::new(texts::gemini_api_key_label())
                .with_placeholder("AIza... or pk-...")
                .with_help_message(texts::generic_api_key_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        };

        let base_url = if let Some(current_url) = current
            .and_then(|v| v.get("env"))
            .and_then(|e| e.get("GOOGLE_GEMINI_BASE_URL"))
            .and_then(|u| u.as_str())
            .filter(|s| !s.is_empty())
        {
            Text::new(texts::gemini_base_url_label())
                .with_initial_value(current_url)
                .with_help_message(texts::gemini_base_url_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        } else {
            Text::new(texts::gemini_base_url_label())
                .with_initial_value(texts::gemini_base_url_placeholder())
                .with_help_message(texts::gemini_base_url_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        };

        let model = if let Some(current_model) = current
            .and_then(|v| v.get("env"))
            .and_then(|e| e.get("GEMINI_MODEL"))
            .and_then(|u| u.as_str())
            .filter(|s| !s.is_empty())
        {
            Text::new(&format!("{}:", texts::model_label()))
                .with_initial_value(current_model)
                .with_help_message(texts::model_default_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        } else {
            Text::new(&format!("{}:", texts::model_label()))
                .with_placeholder("gemini-3-pro-preview")
                .with_help_message(texts::model_default_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        };

        Ok(build_gemini_api_key_settings_config(
            current, &api_key, &base_url, &model,
        ))
    }
}

/// 收集可选字段
pub fn prompt_optional_fields(current: Option<&Provider>) -> Result<OptionalFields, AppError> {
    println!("\n{}", texts::optional_fields_config().bright_cyan().bold());

    let notes = if let Some(provider) = current {
        let initial = provider.notes.as_deref().unwrap_or("");
        Text::new(texts::notes_label())
            .with_initial_value(initial)
            .with_help_message(texts::notes_help_edit())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::notes_label())
            .with_placeholder(texts::notes_example_placeholder())
            .with_help_message(texts::notes_help_new())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };
    let notes = if notes.trim().is_empty() {
        None
    } else {
        Some(notes.trim().to_string())
    };

    let sort_index_str = if let Some(provider) = current {
        let initial = provider
            .sort_index
            .map(|i| i.to_string())
            .unwrap_or_default();
        Text::new(texts::sort_index_label())
            .with_initial_value(&initial)
            .with_help_message(texts::sort_index_help_edit())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::sort_index_label())
            .with_placeholder(texts::sort_index_placeholder())
            .with_help_message(texts::sort_index_help_new())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };
    let sort_index =
        if sort_index_str.trim().is_empty() {
            None
        } else {
            Some(sort_index_str.trim().parse::<usize>().map_err(|_| {
                AppError::InvalidInput(texts::invalid_sort_index_number().to_string())
            })?)
        };

    Ok(OptionalFields {
        notes,
        icon: None,
        icon_color: None,
        sort_index,
    })
}

/// 显示供应商配置摘要
pub fn display_provider_summary(provider: &Provider, app_type: &AppType) {
    println!(
        "\n{}",
        texts::provider_config_summary().bright_green().bold()
    );
    println!("{}: {}", texts::id_label().bright_yellow(), provider.id);
    println!(
        "{}: {}",
        texts::provider_name_label().bright_yellow(),
        provider.name
    );

    if let Some(website) = &provider.website_url {
        println!("{}: {}", texts::website_label().bright_yellow(), website);
    }
    if supports_common_config(app_type) {
        if let Some(enabled) = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.apply_common_config)
        {
            println!(
                "{}: {}",
                texts::tui_form_attach_common_config().bright_yellow(),
                enabled
            );
        }
    }

    // 显示关键配置（不显示完整 API Key）
    println!("\n{}", texts::core_config_label().bright_cyan());
    match app_type {
        AppType::Claude => {
            let is_official = provider
                .category
                .as_deref()
                .is_some_and(|category| category.eq_ignore_ascii_case("official"));
            let is_codex_oauth = provider
                .meta
                .as_ref()
                .and_then(|meta| meta.provider_type.as_deref())
                == Some("codex_oauth");
            if !is_official && !is_codex_oauth {
                let api_format = crate::proxy::providers::get_claude_api_format(provider);
                println!(
                    "  {}: {}",
                    texts::tui_label_claude_api_format(),
                    texts::tui_claude_api_format_value(api_format)
                );
            }
            if is_codex_oauth {
                let account_id = provider
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.managed_account_id_for("codex_oauth"))
                    .unwrap_or_else(|| texts::tui_managed_accounts_follow_default().to_string());
                println!("  {}: {}", texts::tui_label_chatgpt_account(), account_id);
                println!(
                    "  {}: {}",
                    texts::tui_label_codex_fast_mode(),
                    provider.codex_fast_mode_enabled()
                );
            }
            if let Some(env) = provider.settings_config.get("env") {
                if !is_codex_oauth {
                    let api_key_field = ClaudeApiKeyField::from_meta_and_settings(
                        provider.meta.as_ref(),
                        &provider.settings_config,
                    );
                    if let Some(api_key) = env
                        .get(api_key_field.as_env_key())
                        .and_then(|v| v.as_str())
                        .or_else(|| {
                            env.get(api_key_field.alternate_env_key())
                                .and_then(|v| v.as_str())
                        })
                    {
                        println!(
                            "  {}: {}",
                            texts::api_key_display_label(),
                            mask_api_key(api_key)
                        );
                    }
                }
                if let Some(base_url) = env.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) {
                    println!("  {}: {}", texts::base_url_display_label(), base_url);
                }
                if let Some(model) = env.get("ANTHROPIC_MODEL").and_then(|v| v.as_str()) {
                    println!("  {}: {}", texts::model_label(), model);
                }
            }
        }
        AppType::Codex => {
            if !provider.is_codex_official() {
                let api_format =
                    if crate::proxy::providers::codex_provider_uses_chat_completions(provider) {
                        "openai_chat"
                    } else {
                        "openai_responses"
                    };
                println!(
                    "  {}: {}",
                    texts::tui_label_claude_api_format(),
                    texts::tui_codex_api_format_value(api_format)
                );
            }
            if let Some(auth) = provider.settings_config.get("auth") {
                if let Some(api_key) = auth.get("OPENAI_API_KEY").and_then(|v| v.as_str()) {
                    println!(
                        "  {}: {}",
                        texts::api_key_display_label(),
                        mask_api_key(api_key)
                    );
                }
            }
            if let Some(config) = provider
                .settings_config
                .get("config")
                .and_then(|v| v.as_str())
            {
                println!("  {}", texts::config_toml_lines(config.lines().count()));
            }
        }
        AppType::Gemini => {
            if let Some(env) = provider.settings_config.get("env") {
                if let Some(api_key) = env.get("GEMINI_API_KEY").and_then(|v| v.as_str()) {
                    println!(
                        "  {}: {}",
                        texts::api_key_display_label(),
                        mask_api_key(api_key)
                    );
                }
                if let Some(base_url) = env
                    .get("GOOGLE_GEMINI_BASE_URL")
                    .or_else(|| env.get("BASE_URL"))
                    .and_then(|v| v.as_str())
                {
                    println!("  {}: {}", texts::base_url_display_label(), base_url);
                }
            }
        }
        AppType::OpenCode => {
            if let Some(options) = provider.settings_config.get("options") {
                if let Some(api_key) = options.get("apiKey").and_then(|v| v.as_str()) {
                    println!(
                        "  {}: {}",
                        texts::api_key_display_label(),
                        mask_api_key(api_key)
                    );
                }
                if let Some(base_url) = options.get("baseURL").and_then(|v| v.as_str()) {
                    println!("  {}: {}", texts::base_url_display_label(), base_url);
                }
            }
            if let Some(models) = provider
                .settings_config
                .get("models")
                .and_then(|v| v.as_object())
            {
                println!("  {}: {}", texts::model_label(), models.len());
            }
        }
        AppType::Hermes => {
            if let Some(api_key) = provider
                .settings_config
                .get("apiKey")
                .or_else(|| provider.settings_config.get("api_key"))
                .and_then(|v| v.as_str())
            {
                println!(
                    "  {}: {}",
                    texts::api_key_display_label(),
                    mask_api_key(api_key)
                );
            }
            if let Some(base_url) = provider
                .settings_config
                .get("base_url")
                .or_else(|| provider.settings_config.get("baseUrl"))
                .or_else(|| provider.settings_config.get("baseURL"))
                .or_else(|| provider.settings_config.get("endpoint"))
                .and_then(|v| v.as_str())
            {
                println!("  {}: {}", texts::base_url_display_label(), base_url);
            }
            if let Some(model) = provider
                .settings_config
                .get("model")
                .and_then(|v| v.as_str())
            {
                println!("  {}: {}", texts::model_label(), model);
            } else if let Some(models) = provider
                .settings_config
                .get("models")
                .and_then(|v| v.as_object())
            {
                println!("  {}: {}", texts::model_label(), models.len());
            } else if let Some(models) = provider
                .settings_config
                .get("models")
                .and_then(|v| v.as_array())
            {
                println!("  {}: {}", texts::model_label(), models.len());
            }
        }
        AppType::OpenClaw => {
            if let Some(api_key) = provider
                .settings_config
                .get("apiKey")
                .and_then(|v| v.as_str())
            {
                println!(
                    "  {}: {}",
                    texts::api_key_display_label(),
                    mask_api_key(api_key)
                );
            }
            if let Some(base_url) = provider
                .settings_config
                .get("baseUrl")
                .and_then(|v| v.as_str())
            {
                println!("  {}: {}", texts::base_url_display_label(), base_url);
            }
            if let Some(models) = provider
                .settings_config
                .get("models")
                .and_then(|v| v.as_array())
            {
                println!("  {}: {}", texts::model_label(), models.len());
            }
        }
    }

    // 可选字段
    if provider.notes.is_some() || provider.sort_index.is_some() {
        println!("\n{}", texts::optional_fields_label().bright_cyan());
        if let Some(notes) = &provider.notes {
            println!("  {}: {}", texts::notes_label_colon(), notes);
        }
        if let Some(idx) = provider.sort_index {
            println!("  {}: {}", texts::sort_index_label_colon(), idx);
        }
    }

    println!("{}", texts::summary_divider().bright_green().bold());
}

/// 获取当前时间戳（秒）
pub fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// ========== 辅助函数 ==========
/// 检测 Gemini 当前的认证类型
fn detect_gemini_auth_type(value: Option<&Value>) -> Option<String> {
    if let Some(env) = value.and_then(|v| v.get("env")) {
        if env.get("GEMINI_API_KEY").is_some() {
            if env
                .get("GOOGLE_GEMINI_BASE_URL")
                .and_then(|v| v.as_str())
                .map(|s| s.contains("packycode"))
                .unwrap_or(false)
            {
                return Some("packycode".to_string());
            } else {
                return Some("generic".to_string());
            }
        }
    }
    // 如果没有 API Key，假设是 OAuth
    if value
        .and_then(|v| v.get("env"))
        .map(|v| v.as_object().map(|o| o.is_empty()).unwrap_or(true))
        .unwrap_or(true)
    {
        return Some("oauth".to_string());
    }
    None
}

/// 遮蔽 API Key 显示（用于摘要显示）
fn mask_api_key(key: &str) -> String {
    if key.len() <= 8 {
        return "***".to_string();
    }
    format!("{}...{}", &key[..4], &key[key.len() - 4..])
}
