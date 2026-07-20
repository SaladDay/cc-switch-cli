use crate::app_config::AppType;
use crate::provider::{ClaudeApiKeyField, CodexChatReasoningConfig};
use serde_json::{json, Value};

use super::{
    ClaudeApiFormat, CodexModelCatalogField, CodexModelCatalogRow, CodexWireApi, FormMode,
    GeminiAuthType, ProviderAddFormState, HERMES_DEFAULT_API_MODE, OPENCLAW_DEFAULT_API_PROTOCOL,
};

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
enum ProviderTemplateId {
    Custom,
    ClaudeOfficial,
    CodexOAuth,
    OpenAiOfficial,
    DeepSeek,
    GoogleOAuth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProviderTemplateDef {
    id: ProviderTemplateId,
    label: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SponsorProviderPreset {
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
    opencode_base_url: &'static str,
    openclaw_base_url: &'static str,
    hermes_base_url: &'static str,
}

#[cfg(test)]
impl SponsorProviderPreset {
    pub(super) fn id(&self) -> &'static str {
        self.id
    }

    pub(super) fn register_url(&self) -> &'static str {
        self.register_url
    }
}

static SPONSOR_PROVIDER_PRESETS: [SponsorProviderPreset; 8] = [
    SponsorProviderPreset {
        id: "claudeapi",
        provider_name: "ClaudeAPI",
        chip_label: "* ClaudeAPI",
        website_url: "https://console.claudeapi.com",
        register_url: "https://console.claudeapi.com/register?source=cc-switch-cli",
        promo_code: "",
        partner_promotion_key: "claudeapi",
        claude_base_url: "https://gw.claudeapi.com",
        codex_base_url: "",
        gemini_base_url: "",
        opencode_base_url: "",
        openclaw_base_url: "",
        hermes_base_url: "",
    },
    SponsorProviderPreset {
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
        opencode_base_url: "https://www.packyapi.com/v1",
        openclaw_base_url: "https://www.packyapi.com",
        hermes_base_url: "https://www.packyapi.com",
    },
    SponsorProviderPreset {
        id: "cubence",
        provider_name: "Cubence",
        chip_label: "* Cubence",
        website_url: "https://cubence.com",
        register_url: "https://cubence.com/signup?code=SC3M1CAH&source=ccscli",
        promo_code: "CCSCLI",
        partner_promotion_key: "cubence",
        claude_base_url: "https://api.cubence.com",
        codex_base_url: "https://api.cubence.com/v1",
        gemini_base_url: "https://api.cubence.com",
        opencode_base_url: "https://api.cubence.com/v1",
        openclaw_base_url: "https://api.cubence.com",
        hermes_base_url: "https://api.cubence.com",
    },
    SponsorProviderPreset {
        id: "runapi",
        provider_name: "RunAPI",
        chip_label: "* RunAPI",
        website_url: "https://runapi.co",
        register_url: "https://runapi.co/register?aff=kTlB",
        promo_code: "",
        partner_promotion_key: "runapi",
        claude_base_url: "https://runapi.co",
        codex_base_url: "https://runapi.co/v1",
        gemini_base_url: "",
        opencode_base_url: "https://runapi.co",
        openclaw_base_url: "https://runapi.co",
        hermes_base_url: "https://runapi.co",
    },
    SponsorProviderPreset {
        id: "aicodemirror",
        provider_name: "AICodeMirror",
        chip_label: "* AICodeMirror",
        website_url: "https://www.aicodemirror.com",
        register_url: "https://www.aicodemirror.com/register?invitecode=77V9EA",
        promo_code: "",
        partner_promotion_key: "aicodemirror",
        claude_base_url: "https://api.aicodemirror.com/api/claudecode",
        codex_base_url: "https://api.aicodemirror.com/api/codex/backend-api/codex",
        gemini_base_url: "https://api.aicodemirror.com/api/gemini",
        opencode_base_url: "https://api.aicodemirror.com/api/claudecode",
        openclaw_base_url: "https://api.aicodemirror.com/api/claudecode",
        hermes_base_url: "",
    },
    SponsorProviderPreset {
        id: "dds",
        provider_name: "DDS",
        chip_label: "* DDS",
        website_url: "https://www.ddshub.cc",
        register_url: "https://ddshub.short.gy/ccscli",
        promo_code: "",
        partner_promotion_key: "dds",
        claude_base_url: "https://www.ddshub.cc",
        codex_base_url: "https://www.ddshub.cc",
        gemini_base_url: "",
        opencode_base_url: "",
        openclaw_base_url: "",
        hermes_base_url: "",
    },
    SponsorProviderPreset {
        id: "qiniu",
        provider_name: "Qiniu",
        chip_label: "* Qiniu",
        website_url: "https://s.qiniu.com/FVfiEb",
        register_url: "https://s.qiniu.com/FVfiEb",
        promo_code: "",
        partner_promotion_key: "qiniu",
        claude_base_url: "https://api.qnaigc.com",
        codex_base_url: "https://api.qnaigc.com/bypass/openai/v1",
        gemini_base_url: "https://api.qnaigc.com/bypass/vertex",
        opencode_base_url: "https://api.qnaigc.com/v1",
        openclaw_base_url: "https://api.qnaigc.com/v1",
        hermes_base_url: "https://api.qnaigc.com/v1",
    },
    SponsorProviderPreset {
        id: "fenno",
        provider_name: "FennoAI",
        chip_label: "* FennoAI",
        website_url: "https://api.fenno.ai",
        register_url: "https://api.fenno.ai/register?redirect=/purchase?tab=subscription%26group=16&aff=Z6XB52KCVP6Y",
        promo_code: "",
        partner_promotion_key: "fenno",
        claude_base_url: "https://api.fenno.ai",
        codex_base_url: "https://api.fenno.ai",
        gemini_base_url: "",
        opencode_base_url: "https://api.fenno.ai/v1",
        openclaw_base_url: "https://api.fenno.ai/v1",
        hermes_base_url: "https://api.fenno.ai/v1",
    },
];

static SPONSOR_PROVIDER_PRESETS_CLAUDE: [SponsorProviderPreset; 8] = [
    SPONSOR_PROVIDER_PRESETS[0],
    SPONSOR_PROVIDER_PRESETS[6],
    SPONSOR_PROVIDER_PRESETS[7],
    SPONSOR_PROVIDER_PRESETS[3],
    SPONSOR_PROVIDER_PRESETS[2],
    SPONSOR_PROVIDER_PRESETS[1],
    SPONSOR_PROVIDER_PRESETS[4],
    SPONSOR_PROVIDER_PRESETS[5],
];

static SPONSOR_PROVIDER_PRESETS_CODEX: [SponsorProviderPreset; 7] = [
    SPONSOR_PROVIDER_PRESETS[6],
    SPONSOR_PROVIDER_PRESETS[7],
    SPONSOR_PROVIDER_PRESETS[3],
    SPONSOR_PROVIDER_PRESETS[2],
    SPONSOR_PROVIDER_PRESETS[1],
    SPONSOR_PROVIDER_PRESETS[4],
    SPONSOR_PROVIDER_PRESETS[5],
];

static SPONSOR_PROVIDER_PRESETS_GEMINI: [SponsorProviderPreset; 4] = [
    SPONSOR_PROVIDER_PRESETS[6],
    SPONSOR_PROVIDER_PRESETS[2],
    SPONSOR_PROVIDER_PRESETS[1],
    SPONSOR_PROVIDER_PRESETS[4],
];

static SPONSOR_PROVIDER_PRESETS_OPENCODE: [SponsorProviderPreset; 5] = [
    SPONSOR_PROVIDER_PRESETS[6],
    SPONSOR_PROVIDER_PRESETS[7],
    SPONSOR_PROVIDER_PRESETS[3],
    SPONSOR_PROVIDER_PRESETS[2],
    SPONSOR_PROVIDER_PRESETS[4],
];

static SPONSOR_PROVIDER_PRESETS_HERMES: [SponsorProviderPreset; 4] = [
    SPONSOR_PROVIDER_PRESETS[6],
    SPONSOR_PROVIDER_PRESETS[7],
    SPONSOR_PROVIDER_PRESETS[3],
    SPONSOR_PROVIDER_PRESETS[2],
];

static SPONSOR_PROVIDER_PRESETS_OPENCLAW: [SponsorProviderPreset; 5] = [
    SPONSOR_PROVIDER_PRESETS[6],
    SPONSOR_PROVIDER_PRESETS[7],
    SPONSOR_PROVIDER_PRESETS[3],
    SPONSOR_PROVIDER_PRESETS[2],
    SPONSOR_PROVIDER_PRESETS[4],
];

static PROVIDER_TEMPLATE_DEFS_CLAUDE: [ProviderTemplateDef; 3] = [
    ProviderTemplateDef {
        id: ProviderTemplateId::Custom,
        label: "Custom",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::ClaudeOfficial,
        label: "Claude Official",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::CodexOAuth,
        label: "Codex",
    },
];

static PROVIDER_TEMPLATE_DEFS_CODEX: [ProviderTemplateDef; 2] = [
    ProviderTemplateDef {
        id: ProviderTemplateId::Custom,
        label: "Custom",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::OpenAiOfficial,
        label: "OpenAI Official",
    },
];

static PROVIDER_TEMPLATE_DEFS_CODEX_AFTER_SPONSORS: [ProviderTemplateDef; 1] =
    [ProviderTemplateDef {
        id: ProviderTemplateId::DeepSeek,
        label: "DeepSeek",
    }];

static PROVIDER_TEMPLATE_DEFS_GEMINI: [ProviderTemplateDef; 2] = [
    ProviderTemplateDef {
        id: ProviderTemplateId::Custom,
        label: "Custom",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::GoogleOAuth,
        label: "Google OAuth",
    },
];

static PROVIDER_TEMPLATE_DEFS_OPENCODE: [ProviderTemplateDef; 1] = [ProviderTemplateDef {
    id: ProviderTemplateId::Custom,
    label: "Custom",
}];

static PROVIDER_TEMPLATE_DEFS_HERMES: [ProviderTemplateDef; 1] = [ProviderTemplateDef {
    id: ProviderTemplateId::Custom,
    label: "Custom",
}];

static PROVIDER_TEMPLATE_DEFS_OPENCLAW: [ProviderTemplateDef; 1] = [ProviderTemplateDef {
    id: ProviderTemplateId::Custom,
    label: "Custom",
}];

fn runapi_opencode_settings_config(base_url: &str) -> Value {
    json!({
        "npm": "@ai-sdk/anthropic",
        "name": "RunAPI",
        "options": {
            "baseURL": base_url,
            "setCacheKey": true,
        },
        "models": {
            "claude-sonnet-4-6": {
                "name": "Claude Sonnet 4.6",
            },
            "claude-opus-4-8": {
                "name": "Claude Opus 4.8",
            },
            "claude-haiku-4-5": {
                "name": "Claude Haiku 4.5",
            },
        },
    })
}

fn runapi_hermes_models() -> Vec<Value> {
    vec![
        json!({
            "id": "claude-opus-4-8",
            "name": "Claude Opus 4.8",
        }),
        json!({
            "id": "claude-sonnet-4-6",
            "name": "Claude Sonnet 4.6",
        }),
        json!({
            "id": "claude-haiku-4-5",
            "name": "Claude Haiku 4.5",
        }),
    ]
}

fn runapi_openclaw_models() -> Vec<Value> {
    vec![
        json!({
            "id": "claude-opus-4-8",
            "name": "Claude Opus 4.8",
            "contextWindow": 1000000,
        }),
        json!({
            "id": "claude-sonnet-4-6",
            "name": "Claude Sonnet 4.6",
            "contextWindow": 1000000,
        }),
        json!({
            "id": "claude-haiku-4-5",
            "name": "Claude Haiku 4.5",
            "contextWindow": 200000,
        }),
    ]
}

fn qiniu_opencode_settings_config(base_url: &str) -> Value {
    json!({
        "npm": "@ai-sdk/openai-compatible",
        "name": "Qiniu",
        "options": {
            "baseURL": base_url,
            "setCacheKey": true,
        },
        "models": {
            "gpt-5.5": {
                "name": "GPT-5.5",
            },
        },
    })
}

fn qiniu_hermes_models() -> Vec<Value> {
    vec![json!({
        "id": "gpt-5.5",
        "name": "GPT-5.5",
    })]
}

fn qiniu_openclaw_models() -> Vec<Value> {
    vec![json!({
        "id": "gpt-5.5",
        "name": "GPT-5.5",
        "contextWindow": 400000,
    })]
}

fn fenno_opencode_settings_config(base_url: &str) -> Value {
    json!({
        "npm": "@ai-sdk/openai-compatible",
        "name": "FennoAI",
        "options": {
            "baseURL": base_url,
            "setCacheKey": true,
        },
        "models": {
            "gpt-5.5": {
                "name": "GPT-5.5",
            },
        },
    })
}

fn fenno_hermes_models() -> Vec<Value> {
    vec![json!({
        "id": "gpt-5.5",
        "name": "GPT-5.5",
    })]
}

fn fenno_openclaw_models() -> Vec<Value> {
    vec![json!({
        "id": "gpt-5.5",
        "name": "GPT-5.5",
        "contextWindow": 400000,
    })]
}

pub(super) fn provider_builtin_template_defs(app_type: &AppType) -> &'static [ProviderTemplateDef] {
    match app_type {
        AppType::Claude => &PROVIDER_TEMPLATE_DEFS_CLAUDE,
        AppType::Codex => &PROVIDER_TEMPLATE_DEFS_CODEX,
        AppType::Gemini => &PROVIDER_TEMPLATE_DEFS_GEMINI,
        AppType::OpenCode => &PROVIDER_TEMPLATE_DEFS_OPENCODE,
        AppType::Hermes => &PROVIDER_TEMPLATE_DEFS_HERMES,
        AppType::OpenClaw => &PROVIDER_TEMPLATE_DEFS_OPENCLAW,
    }
}

pub(super) fn provider_sponsor_presets(app_type: &AppType) -> &'static [SponsorProviderPreset] {
    match app_type {
        AppType::Claude => &SPONSOR_PROVIDER_PRESETS_CLAUDE,
        AppType::Codex => &SPONSOR_PROVIDER_PRESETS_CODEX,
        AppType::Gemini => &SPONSOR_PROVIDER_PRESETS_GEMINI,
        AppType::OpenCode => &SPONSOR_PROVIDER_PRESETS_OPENCODE,
        AppType::Hermes => &SPONSOR_PROVIDER_PRESETS_HERMES,
        AppType::OpenClaw => &SPONSOR_PROVIDER_PRESETS_OPENCLAW,
    }
}

pub(super) fn provider_after_sponsor_template_defs(
    app_type: &AppType,
) -> &'static [ProviderTemplateDef] {
    match app_type {
        AppType::Codex => &PROVIDER_TEMPLATE_DEFS_CODEX_AFTER_SPONSORS,
        AppType::Claude
        | AppType::Gemini
        | AppType::OpenCode
        | AppType::Hermes
        | AppType::OpenClaw => &[],
    }
}

impl ProviderAddFormState {
    pub fn template_count(&self) -> usize {
        provider_builtin_template_defs(&self.app_type).len()
            + provider_sponsor_presets(&self.app_type).len()
            + provider_after_sponsor_template_defs(&self.app_type).len()
    }

    pub fn template_labels(&self) -> Vec<&'static str> {
        let mut labels = provider_builtin_template_defs(&self.app_type)
            .iter()
            .map(|def| def.label)
            .collect::<Vec<_>>();
        labels.extend(
            provider_sponsor_presets(&self.app_type)
                .iter()
                .map(|preset| preset.chip_label),
        );
        labels.extend(
            provider_after_sponsor_template_defs(&self.app_type)
                .iter()
                .map(|def| def.label),
        );
        labels
    }

    pub fn apply_template(&mut self, idx: usize, existing_ids: &[String]) {
        let builtin_defs = provider_builtin_template_defs(&self.app_type);
        let sponsor_presets = provider_sponsor_presets(&self.app_type);
        let after_sponsor_defs = provider_after_sponsor_template_defs(&self.app_type);
        let total_templates = builtin_defs.len() + sponsor_presets.len() + after_sponsor_defs.len();
        let idx = idx.min(total_templates.saturating_sub(1));
        self.template_idx = idx;
        self.field_errors.clear();
        self.usage_query_field_errors.clear();
        self.clear_text_edit();
        self.id_is_manual = false;
        self.reset_local_proxy_settings_state();

        if idx >= builtin_defs.len() && idx < builtin_defs.len() + sponsor_presets.len() {
            let sponsor_idx = idx.saturating_sub(builtin_defs.len());
            if let Some(preset) = sponsor_presets.get(sponsor_idx) {
                self.apply_sponsor_preset(preset);
            }
        } else {
            let template_id = if idx < builtin_defs.len() {
                builtin_defs
                    .get(idx)
                    .map(|def| def.id)
                    .unwrap_or(ProviderTemplateId::Custom)
            } else {
                let after_sponsor_idx =
                    idx.saturating_sub(builtin_defs.len() + sponsor_presets.len());
                after_sponsor_defs
                    .get(after_sponsor_idx)
                    .map(|def| def.id)
                    .unwrap_or(ProviderTemplateId::Custom)
            };

            if template_id == ProviderTemplateId::Custom {
                if matches!(self.mode, FormMode::Add) {
                    let defaults = Self::new(self.app_type.clone());
                    let previous_include_common_config = self.include_common_config;
                    let previous_include_common_config_touched = self.include_common_config_touched;
                    self.extra = defaults.extra;
                    self.id = defaults.id;
                    self.id_is_manual = defaults.id_is_manual;
                    self.name = defaults.name;
                    self.website_url = defaults.website_url;
                    self.notes = defaults.notes;
                    self.include_common_config = previous_include_common_config;
                    self.include_common_config_touched = previous_include_common_config_touched;
                    self.json_scroll = defaults.json_scroll;
                    self.codex_preview_section = defaults.codex_preview_section;
                    self.codex_auth_scroll = defaults.codex_auth_scroll;
                    self.codex_config_scroll = defaults.codex_config_scroll;
                    self.claude_model_config_touched = defaults.claude_model_config_touched;
                    self.claude_api_key = defaults.claude_api_key;
                    self.claude_api_key_field = defaults.claude_api_key_field;
                    self.claude_base_url = defaults.claude_base_url;
                    self.claude_api_format = defaults.claude_api_format;
                    self.claude_model = defaults.claude_model;
                    self.claude_haiku_model = defaults.claude_haiku_model;
                    self.claude_sonnet_model = defaults.claude_sonnet_model;
                    self.claude_opus_model = defaults.claude_opus_model;
                    self.claude_sonnet_one_m = defaults.claude_sonnet_one_m;
                    self.claude_opus_one_m = defaults.claude_opus_one_m;
                    self.claude_hide_attribution = defaults.claude_hide_attribution;
                    self.claude_teammates = defaults.claude_teammates;
                    self.claude_tool_search = defaults.claude_tool_search;
                    self.claude_disable_auto_upgrade = defaults.claude_disable_auto_upgrade;
                    self.codex_oauth_account_id = defaults.codex_oauth_account_id;
                    self.codex_fast_mode = defaults.codex_fast_mode;
                    self.codex_base_url = defaults.codex_base_url;
                    self.codex_model = defaults.codex_model;
                    self.codex_wire_api = defaults.codex_wire_api;
                    self.codex_requires_openai_auth = defaults.codex_requires_openai_auth;
                    self.codex_env_key = defaults.codex_env_key;
                    self.codex_api_key = defaults.codex_api_key;
                    self.codex_chat_reasoning = defaults.codex_chat_reasoning;
                    self.codex_model_catalog = defaults.codex_model_catalog;
                    self.codex_local_routing_enabled = defaults.codex_local_routing_enabled;
                    self.codex_goal_mode = defaults.codex_goal_mode;
                    self.codex_remote_compaction = defaults.codex_remote_compaction;
                    self.codex_local_routing_field_idx = defaults.codex_local_routing_field_idx;
                    self.codex_model_catalog_idx = defaults.codex_model_catalog_idx;
                    self.codex_model_catalog_field = defaults.codex_model_catalog_field;
                    self.gemini_auth_type = defaults.gemini_auth_type;
                    self.gemini_api_key = defaults.gemini_api_key;
                    self.gemini_base_url = defaults.gemini_base_url;
                    self.gemini_model = defaults.gemini_model;
                    self.openclaw_user_agent = defaults.openclaw_user_agent;
                    self.openclaw_models = defaults.openclaw_models;
                    self.hermes_api_mode = defaults.hermes_api_mode;
                    self.hermes_api_key = defaults.hermes_api_key;
                    self.hermes_base_url = defaults.hermes_base_url;
                    self.hermes_models = defaults.hermes_models;
                    self.hermes_rate_limit_delay = defaults.hermes_rate_limit_delay;
                    self.opencode_npm_package = defaults.opencode_npm_package;
                    self.opencode_api_key = defaults.opencode_api_key;
                    self.opencode_base_url = defaults.opencode_base_url;
                    self.opencode_model_id = defaults.opencode_model_id;
                    self.opencode_model_name = defaults.opencode_model_name;
                    self.opencode_model_context_limit = defaults.opencode_model_context_limit;
                    self.opencode_model_output_limit = defaults.opencode_model_output_limit;
                    self.opencode_model_original_id = defaults.opencode_model_original_id;
                }
                return;
            }

            self.extra = json!({});
            self.notes.set("");
            match template_id {
                ProviderTemplateId::Custom => {}
                ProviderTemplateId::ClaudeOfficial => {
                    self.extra = json!({
                        "category": "official",
                    });
                    self.name.set("Claude Official");
                    self.website_url
                        .set("https://www.anthropic.com/claude-code");
                    self.claude_api_key.set("");
                    self.claude_api_key_field = ClaudeApiKeyField::AuthToken;
                    self.claude_base_url.set("");
                    self.claude_api_format = ClaudeApiFormat::Anthropic;
                    self.claude_model.set("");
                    self.claude_haiku_model.set("");
                    self.claude_sonnet_model.set("");
                    self.claude_opus_model.set("");
                    self.claude_sonnet_one_m = false;
                    self.claude_opus_one_m = false;
                    self.claude_model_config_touched = false;
                    self.codex_oauth_account_id = None;
                    self.codex_fast_mode = false;
                    self.claude_hide_attribution = false;
                    self.claude_hide_attribution_touched = false;
                    self.claude_teammates = false;
                    self.claude_teammates_touched = false;
                    self.claude_tool_search = false;
                    self.claude_tool_search_touched = false;
                    self.claude_disable_auto_upgrade = false;
                    self.claude_disable_auto_upgrade_touched = false;
                }
                ProviderTemplateId::CodexOAuth => {
                    self.extra = json!({
                        "meta": {
                            "providerType": "codex_oauth",
                            "authBinding": {
                                "source": "managed_account",
                                "authProvider": "codex_oauth",
                            },
                        }
                    });
                    self.name.set("Codex");
                    self.website_url.set("https://openai.com/chatgpt/pricing");
                    self.claude_api_key.set("");
                    self.claude_api_key_field = ClaudeApiKeyField::AuthToken;
                    self.claude_base_url
                        .set("https://chatgpt.com/backend-api/codex");
                    self.claude_api_format = ClaudeApiFormat::OpenAiResponses;
                    self.claude_model.set("gpt-5.4");
                    self.claude_haiku_model.set("gpt-5.4-mini");
                    self.claude_sonnet_model.set("gpt-5.4");
                    self.claude_opus_model.set("gpt-5.4");
                    self.claude_sonnet_one_m = false;
                    self.claude_opus_one_m = false;
                    self.claude_model_config_touched = true;
                    self.codex_oauth_account_id = None;
                    self.codex_fast_mode = false;
                    self.claude_hide_attribution = true;
                    self.claude_hide_attribution_touched = true;
                    self.claude_teammates = false;
                    self.claude_teammates_touched = false;
                    self.claude_tool_search = false;
                    self.claude_tool_search_touched = false;
                    self.claude_disable_auto_upgrade = false;
                    self.claude_disable_auto_upgrade_touched = false;
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
                    self.codex_api_key.set("");
                    self.codex_base_url.set("");
                    self.codex_model.set("");
                    self.codex_wire_api = CodexWireApi::Responses;
                    self.codex_requires_openai_auth = true;
                    self.codex_env_key.set("");
                    self.reset_codex_local_routing_state();
                }
                ProviderTemplateId::DeepSeek => {
                    self.extra = json!({
                        "category": "cn_official",
                        "icon": "deepseek",
                        "iconColor": "#1E88E5",
                        "meta": {
                            "apiFormat": "openai_chat",
                            "codexChatReasoning": {
                                "supportsThinking": true,
                                "supportsEffort": true,
                                "thinkingParam": "thinking",
                                "effortParam": "reasoning_effort",
                                "effortValueMode": "deepseek",
                                "outputFormat": "reasoning_content",
                            },
                        },
                        "settingsConfig": {
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
                        },
                    });
                    self.name.set("DeepSeek");
                    self.website_url.set("https://platform.deepseek.com");
                    self.codex_api_key.set("");
                    self.codex_base_url.set("https://api.deepseek.com");
                    self.codex_model.set("deepseek-v4-flash");
                    self.codex_wire_api = CodexWireApi::Responses;
                    self.codex_requires_openai_auth = true;
                    self.codex_env_key.set("");
                    self.claude_api_format = ClaudeApiFormat::OpenAiChat;
                    self.codex_chat_reasoning = CodexChatReasoningConfig {
                        supports_thinking: Some(true),
                        supports_effort: Some(true),
                        thinking_param: Some("thinking".to_string()),
                        effort_param: Some("reasoning_effort".to_string()),
                        effort_value_mode: Some("deepseek".to_string()),
                        output_format: Some("reasoning_content".to_string()),
                    };
                    self.codex_model_catalog = vec![
                        CodexModelCatalogRow {
                            model: "deepseek-v4-flash".to_string(),
                            display_name: "DeepSeek V4 Flash".to_string(),
                            context_window: "1000000".to_string(),
                        },
                        CodexModelCatalogRow {
                            model: "deepseek-v4-pro".to_string(),
                            display_name: "DeepSeek V4 Pro".to_string(),
                            context_window: "1000000".to_string(),
                        },
                    ];
                    self.codex_local_routing_field_idx = 0;
                    self.codex_model_catalog_idx = 0;
                    self.codex_model_catalog_field = CodexModelCatalogField::Model;
                }
                ProviderTemplateId::GoogleOAuth => {
                    self.extra = json!({
                        "category": "official",
                        "meta": {
                            "partnerPromotionKey": "google-official",
                        }
                    });
                    self.name.set("Google OAuth");
                    self.website_url.set("https://ai.google.dev");
                    self.gemini_auth_type = GeminiAuthType::OAuth;
                }
            };
        }

        // A preset with a model catalog implies routing/mapping is on (no
        // dedicated stored field), matching the load-time initialization.
        if matches!(self.app_type, AppType::Codex) {
            self.codex_local_routing_enabled = !self.codex_model_catalog.is_empty();
        }

        if !self.id_is_manual && !self.name.is_blank() {
            let id = crate::cli::commands::provider_input::generate_provider_id_for_app(
                &self.app_type,
                self.name.value.trim(),
                existing_ids,
            );
            self.id.set(id);
        }
    }

    fn apply_sponsor_preset(&mut self, preset: &SponsorProviderPreset) {
        let mut extra = json!({
            "meta": {
                "isPartner": true,
                "partnerPromotionKey": preset.partner_promotion_key,
            }
        });
        if preset.id == "runapi" {
            if let Some(obj) = extra.as_object_mut() {
                obj.insert("category".to_string(), json!("aggregator"));
                obj.insert("icon".to_string(), json!("runapi"));
            }
        } else if preset.id == "qiniu" {
            if let Some(obj) = extra.as_object_mut() {
                obj.insert("category".to_string(), json!("aggregator"));
                obj.insert("icon".to_string(), json!("qiniu"));
            }
        } else if preset.id == "fenno" {
            if let Some(obj) = extra.as_object_mut() {
                obj.insert("category".to_string(), json!("aggregator"));
                obj.insert("icon".to_string(), json!("fenno"));
            }
        }
        self.extra = extra;
        self.name.set(preset.provider_name);
        self.website_url.set(preset.website_url);
        self.notes.set("");

        match self.app_type {
            AppType::Claude => {
                self.claude_api_key_field = ClaudeApiKeyField::AuthToken;
                self.claude_base_url.set(preset.claude_base_url);
            }
            AppType::Codex => {
                self.codex_base_url.set(preset.codex_base_url);
                self.codex_model
                    .set(if preset.id == "qiniu" || preset.id == "fenno" {
                        "gpt-5.5"
                    } else {
                        "gpt-5.4"
                    });
                self.codex_wire_api = CodexWireApi::Responses;
                self.codex_requires_openai_auth = true;
                self.reset_codex_local_routing_state();
            }
            AppType::Gemini => {
                self.gemini_auth_type = GeminiAuthType::ApiKey;
                self.gemini_base_url.set(preset.gemini_base_url);
                self.gemini_model.set(if preset.id == "qiniu" {
                    "gemini-3.1-pro-preview"
                } else {
                    ""
                });
            }
            AppType::OpenCode => {
                if preset.id == "aicodemirror" {
                    self.extra["settingsConfig"] = json!({
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
                    });
                    self.opencode_npm_package.set("@ai-sdk/anthropic");
                    self.opencode_api_key.set("");
                    self.opencode_base_url.set(preset.claude_base_url);
                    self.opencode_model_id.set("claude-opus-4.6");
                    self.opencode_model_name.set("Claude Opus 4.6");
                    self.opencode_model_context_limit.set("");
                    self.opencode_model_output_limit.set("");
                    self.opencode_model_original_id = Some("claude-opus-4.6".to_string());
                } else if preset.id == "runapi" {
                    self.extra["settingsConfig"] =
                        runapi_opencode_settings_config(preset.opencode_base_url);
                    self.opencode_npm_package.set("@ai-sdk/anthropic");
                    self.opencode_api_key.set("");
                    self.opencode_base_url.set(preset.opencode_base_url);
                    self.opencode_model_id.set("claude-sonnet-4-6");
                    self.opencode_model_name.set("Claude Sonnet 4.6");
                    self.opencode_model_context_limit.set("");
                    self.opencode_model_output_limit.set("");
                    self.opencode_model_original_id = Some("claude-sonnet-4-6".to_string());
                } else if preset.id == "qiniu" {
                    self.extra["settingsConfig"] =
                        qiniu_opencode_settings_config(preset.opencode_base_url);
                    self.opencode_npm_package.set("@ai-sdk/openai-compatible");
                    self.opencode_api_key.set("");
                    self.opencode_base_url.set(preset.opencode_base_url);
                    self.opencode_model_id.set("gpt-5.5");
                    self.opencode_model_name.set("GPT-5.5");
                    self.opencode_model_context_limit.set("");
                    self.opencode_model_output_limit.set("");
                    self.opencode_model_original_id = Some("gpt-5.5".to_string());
                } else if preset.id == "fenno" {
                    self.extra["settingsConfig"] =
                        fenno_opencode_settings_config(preset.opencode_base_url);
                    self.opencode_npm_package.set("@ai-sdk/openai-compatible");
                    self.opencode_api_key.set("");
                    self.opencode_base_url.set(preset.opencode_base_url);
                    self.opencode_model_id.set("gpt-5.5");
                    self.opencode_model_name.set("GPT-5.5");
                    self.opencode_model_context_limit.set("");
                    self.opencode_model_output_limit.set("");
                    self.opencode_model_original_id = Some("gpt-5.5".to_string());
                } else {
                    self.opencode_npm_package.set("@ai-sdk/openai-compatible");
                    self.opencode_api_key.set("");
                    self.opencode_base_url.set(preset.opencode_base_url);
                    self.opencode_model_id.set("");
                    self.opencode_model_name.set("");
                    self.opencode_model_context_limit.set("");
                    self.opencode_model_output_limit.set("");
                    self.opencode_model_original_id = None;
                }
            }
            AppType::Hermes => {
                if preset.id == "runapi" {
                    self.extra["settingsConfig"] = json!({
                        "name": "runapi",
                    });
                    self.hermes_api_mode = "anthropic_messages".to_string();
                    self.hermes_models = runapi_hermes_models();
                } else if preset.id == "qiniu" {
                    self.extra["settingsConfig"] = json!({
                        "name": "qiniu",
                    });
                    self.hermes_api_mode = HERMES_DEFAULT_API_MODE.to_string();
                    self.hermes_models = qiniu_hermes_models();
                } else if preset.id == "fenno" {
                    self.extra["settingsConfig"] = json!({
                        "name": "fenno",
                    });
                    self.hermes_api_mode = HERMES_DEFAULT_API_MODE.to_string();
                    self.hermes_models = fenno_hermes_models();
                } else {
                    self.hermes_api_mode = HERMES_DEFAULT_API_MODE.to_string();
                    self.hermes_models = Vec::new();
                }
                self.hermes_api_key.set("");
                self.hermes_base_url.set(preset.hermes_base_url);
                self.hermes_rate_limit_delay.set("");
            }
            AppType::OpenClaw => {
                if preset.id == "aicodemirror" {
                    self.opencode_api_key.set("");
                    self.opencode_base_url.set(preset.claude_base_url);
                    self.opencode_npm_package.set("anthropic-messages");
                    self.openclaw_user_agent = false;
                    self.openclaw_models = vec![
                        json!({
                            "id": "claude-opus-4-6",
                            "name": "Claude Opus 4.6",
                            "contextWindow": 200000,
                            "cost": {
                                "input": 5,
                                "output": 25,
                            },
                        }),
                        json!({
                            "id": "claude-sonnet-4-6",
                            "name": "Claude Sonnet 4.6",
                            "contextWindow": 200000,
                            "cost": {
                                "input": 3,
                                "output": 15,
                            },
                        }),
                    ];
                    self.opencode_model_id.set("claude-opus-4-6");
                    self.opencode_model_name.set("Claude Opus 4.6");
                    self.opencode_model_context_limit.set("200000");
                    self.opencode_model_original_id = Some("claude-opus-4-6".to_string());
                } else if preset.id == "runapi" {
                    self.opencode_api_key.set("");
                    self.opencode_base_url.set(preset.openclaw_base_url);
                    self.opencode_npm_package.set("anthropic-messages");
                    self.openclaw_user_agent = false;
                    self.openclaw_models = runapi_openclaw_models();
                    self.opencode_model_id.set("claude-sonnet-4-6");
                    self.opencode_model_name.set("Claude Sonnet 4.6");
                    self.opencode_model_context_limit.set("1000000");
                    self.opencode_model_output_limit.set("");
                    self.opencode_model_original_id = Some("claude-sonnet-4-6".to_string());
                } else if preset.id == "qiniu" {
                    self.opencode_api_key.set("");
                    self.opencode_base_url.set(preset.openclaw_base_url);
                    self.opencode_npm_package.set(OPENCLAW_DEFAULT_API_PROTOCOL);
                    self.openclaw_user_agent = false;
                    self.openclaw_models = qiniu_openclaw_models();
                    self.opencode_model_id.set("gpt-5.5");
                    self.opencode_model_name.set("GPT-5.5");
                    self.opencode_model_context_limit.set("400000");
                    self.opencode_model_output_limit.set("");
                    self.opencode_model_original_id = Some("gpt-5.5".to_string());
                } else if preset.id == "fenno" {
                    self.opencode_api_key.set("");
                    self.opencode_base_url.set(preset.openclaw_base_url);
                    self.opencode_npm_package.set(OPENCLAW_DEFAULT_API_PROTOCOL);
                    self.openclaw_user_agent = false;
                    self.openclaw_models = fenno_openclaw_models();
                    self.opencode_model_id.set("gpt-5.5");
                    self.opencode_model_name.set("GPT-5.5");
                    self.opencode_model_context_limit.set("400000");
                    self.opencode_model_output_limit.set("");
                    self.opencode_model_original_id = Some("gpt-5.5".to_string());
                } else {
                    self.opencode_api_key.set("");
                    self.opencode_base_url.set(preset.openclaw_base_url);
                    self.opencode_npm_package.set(OPENCLAW_DEFAULT_API_PROTOCOL);
                    self.openclaw_user_agent = false;
                    self.openclaw_models = Vec::new();
                    self.opencode_model_id.set("");
                    self.opencode_model_name.set("");
                    self.opencode_model_context_limit.set("");
                    self.opencode_model_output_limit.set("");
                    self.opencode_model_original_id = None;
                }
            }
        }

        if matches!(self.app_type, AppType::Codex) {
            self.codex_local_routing_enabled = !self.codex_model_catalog.is_empty();
        }
    }

    fn reset_codex_local_routing_state(&mut self) {
        self.claude_api_format = ClaudeApiFormat::OpenAiResponses;
        self.codex_chat_reasoning = CodexChatReasoningConfig::default();
        self.codex_model_catalog.clear();
        self.codex_local_routing_enabled = false;
        self.codex_local_routing_field_idx = 0;
        self.codex_model_catalog_idx = 0;
        self.codex_model_catalog_field = CodexModelCatalogField::Model;
    }
}
