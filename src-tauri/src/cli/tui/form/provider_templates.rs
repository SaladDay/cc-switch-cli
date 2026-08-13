use crate::app_config::AppType;
use crate::provider::{ClaudeApiKeyField, CodexChatReasoningConfig};
use crate::provider_preset_models::{
    codex_oauth_claude_env, sponsor_hermes_models, sponsor_model_family, sponsor_openclaw_models,
    sponsor_opencode_settings, SponsorModelFamily, CODEX_DEFAULT_MODEL, CODEX_OAUTH_FAST_MODEL,
    GEMINI_DEFAULT_MODEL,
};
use crate::provider_preset_sponsors::{sponsor_provider_presets_for_app, SponsorProviderPreset};
use serde_json::json;

use super::{
    ClaudeApiFormat, CodexModelCatalogField, CodexModelCatalogRow, CodexWireApi, FormMode,
    GeminiAuthType, PromptCacheRoutingMode, ProviderAddFormState, HERMES_DEFAULT_API_MODE,
    OPENCLAW_DEFAULT_API_PROTOCOL,
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

#[cfg(test)]
impl SponsorProviderPreset {
    pub(super) fn id(&self) -> &'static str {
        self.id
    }

    pub(super) fn register_url(&self) -> &'static str {
        self.register_url
    }
}

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
    sponsor_provider_presets_for_app(app_type)
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
    fn reset_claude_template_state(&mut self) {
        self.claude_api_key.set("");
        self.claude_api_key_field = ClaudeApiKeyField::AuthToken;
        self.claude_base_url.set("");
        self.claude_api_format = ClaudeApiFormat::Anthropic;
        self.claude_model.set("");
        self.claude_haiku_model.set("");
        self.claude_sonnet_model.set("");
        self.claude_opus_model.set("");
        self.claude_fable_model.set("");
        self.claude_subagent_model.set("");
        self.claude_sonnet_one_m = false;
        self.claude_opus_one_m = false;
        self.claude_fable_one_m = false;
        self.claude_subagent_one_m = false;
        self.claude_fallback_model_touched = false;
        self.claude_model_role_touched.fill(false);
        self.claude_hide_attribution = false;
        self.claude_hide_attribution_touched = false;
        self.claude_teammates = false;
        self.claude_teammates_touched = false;
        self.claude_tool_search = false;
        self.claude_tool_search_touched = false;
        self.claude_effort_max = false;
        self.claude_effort_max_touched = false;
        self.claude_disable_auto_upgrade = false;
        self.claude_disable_auto_upgrade_touched = false;
        self.claude_quick_config_idx = 0;
        self.codex_oauth_account_id = None;
        self.codex_fast_mode = false;
    }

    fn reset_codex_template_state(&mut self) {
        self.codex_api_key.set("");
        self.codex_base_url.set("");
        self.codex_model.set(CODEX_DEFAULT_MODEL);
        self.codex_wire_api = CodexWireApi::Responses;
        self.codex_requires_openai_auth = true;
        self.codex_env_key.set("OPENAI_API_KEY");
        self.codex_goal_mode = false;
        self.codex_goal_mode_touched = false;
        self.codex_remote_compaction = false;
        self.codex_remote_compaction_touched = false;
        self.codex_quick_config_idx = 0;
        self.reset_codex_local_routing_state();
    }

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
        self.is_full_url = false;
        if matches!(self.app_type, AppType::Codex) {
            self.codex_prompt_cache_routing = PromptCacheRoutingMode::Auto;
        }

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
                    self.claude_fallback_model_touched = defaults.claude_fallback_model_touched;
                    self.claude_model_role_touched = defaults.claude_model_role_touched;
                    self.claude_api_key = defaults.claude_api_key;
                    self.claude_api_key_field = defaults.claude_api_key_field;
                    self.claude_base_url = defaults.claude_base_url;
                    self.claude_api_format = defaults.claude_api_format;
                    self.claude_model = defaults.claude_model;
                    self.claude_haiku_model = defaults.claude_haiku_model;
                    self.claude_sonnet_model = defaults.claude_sonnet_model;
                    self.claude_opus_model = defaults.claude_opus_model;
                    self.claude_fable_model = defaults.claude_fable_model;
                    self.claude_subagent_model = defaults.claude_subagent_model;
                    self.claude_sonnet_one_m = defaults.claude_sonnet_one_m;
                    self.claude_opus_one_m = defaults.claude_opus_one_m;
                    self.claude_fable_one_m = defaults.claude_fable_one_m;
                    self.claude_subagent_one_m = defaults.claude_subagent_one_m;
                    self.claude_hide_attribution = defaults.claude_hide_attribution;
                    self.claude_teammates = defaults.claude_teammates;
                    self.claude_tool_search = defaults.claude_tool_search;
                    self.claude_effort_max = defaults.claude_effort_max;
                    self.claude_disable_auto_upgrade = defaults.claude_disable_auto_upgrade;
                    self.codex_oauth_account_id = defaults.codex_oauth_account_id;
                    self.codex_fast_mode = defaults.codex_fast_mode;
                    self.codex_impersonate_claude_code = defaults.codex_impersonate_claude_code;
                    self.codex_max_output_tokens = defaults.codex_max_output_tokens;
                    self.codex_base_url = defaults.codex_base_url;
                    self.codex_model = defaults.codex_model;
                    self.codex_wire_api = defaults.codex_wire_api;
                    self.codex_requires_openai_auth = defaults.codex_requires_openai_auth;
                    self.codex_env_key = defaults.codex_env_key;
                    self.codex_api_key = defaults.codex_api_key;
                    self.codex_chat_reasoning = defaults.codex_chat_reasoning;
                    self.codex_prompt_cache_routing = defaults.codex_prompt_cache_routing;
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

            if matches!(self.app_type, AppType::Codex) {
                self.reset_codex_template_state();
            }
            self.extra = json!({});
            self.notes.set("");
            self.codex_impersonate_claude_code = false;
            self.codex_max_output_tokens.set("");
            match template_id {
                ProviderTemplateId::Custom => {}
                ProviderTemplateId::ClaudeOfficial => {
                    self.reset_claude_template_state();
                    self.extra = json!({
                        "category": "official",
                    });
                    self.name.set("Claude Official");
                    self.website_url
                        .set("https://www.anthropic.com/claude-code");
                }
                ProviderTemplateId::CodexOAuth => {
                    self.reset_claude_template_state();
                    self.extra = json!({
                        "meta": {
                            "providerType": "codex_oauth",
                            "authBinding": {
                                "source": "managed_account",
                                "authProvider": "codex_oauth",
                            },
                        },
                        "settingsConfig": {
                            "env": codex_oauth_claude_env(),
                        },
                    });
                    self.name.set("Codex");
                    self.website_url.set("https://openai.com/chatgpt/pricing");
                    self.claude_base_url
                        .set("https://chatgpt.com/backend-api/codex");
                    self.claude_api_format = ClaudeApiFormat::OpenAiResponses;
                    self.claude_model.set(CODEX_DEFAULT_MODEL);
                    self.claude_haiku_model.set(CODEX_OAUTH_FAST_MODEL);
                    self.claude_sonnet_model.set(CODEX_DEFAULT_MODEL);
                    self.claude_opus_model.set(CODEX_DEFAULT_MODEL);
                    self.claude_hide_attribution = true;
                    self.claude_hide_attribution_touched = true;
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
                            supports_parallel_tool_calls: None,
                            input_modalities: Vec::new(),
                            base_instructions: String::new(),
                        },
                        CodexModelCatalogRow {
                            model: "deepseek-v4-pro".to_string(),
                            display_name: "DeepSeek V4 Pro".to_string(),
                            context_window: "1000000".to_string(),
                            supports_parallel_tool_calls: None,
                            input_modalities: Vec::new(),
                            base_instructions: String::new(),
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
        if preset.id == "runapi" || preset.id == "openmodel" {
            if let Some(obj) = extra.as_object_mut() {
                obj.insert("category".to_string(), json!("aggregator"));
                if preset.id == "runapi" {
                    obj.insert("icon".to_string(), json!("runapi"));
                }
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
                self.reset_claude_template_state();
                self.claude_base_url.set(preset.claude_base_url);
            }
            AppType::Codex => {
                self.reset_codex_template_state();
                self.codex_base_url.set(preset.codex_base_url);
            }
            AppType::Gemini => {
                self.gemini_auth_type = GeminiAuthType::ApiKey;
                self.gemini_api_key.set("");
                self.gemini_base_url.set(preset.gemini_base_url);
                self.gemini_model.set(GEMINI_DEFAULT_MODEL);
            }
            AppType::OpenCode => {
                let family = sponsor_model_family(preset.id);
                if let Some(family) = family {
                    self.extra["settingsConfig"] = sponsor_opencode_settings(
                        preset.provider_name,
                        preset.opencode_base_url,
                        family,
                    );
                    self.opencode_npm_package.set(match family {
                        SponsorModelFamily::Claude | SponsorModelFamily::RunApiClaude => {
                            "@ai-sdk/anthropic"
                        }
                        SponsorModelFamily::Gpt => "@ai-sdk/openai-compatible",
                    });
                    self.opencode_api_key.set("");
                    self.opencode_base_url.set(preset.opencode_base_url);
                    self.opencode_model_id.set(family.primary_model());
                    self.opencode_model_name.set(family.primary_model_name());
                    self.opencode_model_context_limit.set("");
                    self.opencode_model_output_limit.set("");
                    self.opencode_model_original_id = Some(family.primary_model().to_string());
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
                let family = sponsor_model_family(preset.id);
                if let Some(family) = family {
                    self.extra["settingsConfig"] = json!({
                        "name": preset.partner_promotion_key,
                    });
                    self.hermes_api_mode = match family {
                        SponsorModelFamily::Claude | SponsorModelFamily::RunApiClaude => {
                            "anthropic_messages"
                        }
                        SponsorModelFamily::Gpt => HERMES_DEFAULT_API_MODE,
                    }
                    .to_string();
                    self.hermes_models = sponsor_hermes_models(family);
                } else {
                    self.hermes_api_mode = HERMES_DEFAULT_API_MODE.to_string();
                    self.hermes_models = Vec::new();
                }
                self.hermes_api_key.set("");
                self.hermes_base_url.set(preset.hermes_base_url);
                self.hermes_rate_limit_delay.set("");
            }
            AppType::OpenClaw => {
                let family = sponsor_model_family(preset.id);
                if let Some(family) = family {
                    self.opencode_api_key.set("");
                    self.opencode_base_url.set(preset.openclaw_base_url);
                    self.opencode_npm_package.set(match family {
                        SponsorModelFamily::Claude | SponsorModelFamily::RunApiClaude => {
                            "anthropic-messages"
                        }
                        SponsorModelFamily::Gpt => OPENCLAW_DEFAULT_API_PROTOCOL,
                    });
                    self.openclaw_user_agent = false;
                    self.openclaw_models = sponsor_openclaw_models(family);
                    self.opencode_model_id.set(family.primary_model());
                    self.opencode_model_name.set(family.primary_model_name());
                    self.opencode_model_context_limit
                        .set(family.primary_context_window());
                    self.opencode_model_output_limit.set("");
                    self.opencode_model_original_id = Some(family.primary_model().to_string());
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
        self.claude_api_key_field = ClaudeApiKeyField::AuthToken;
        self.codex_impersonate_claude_code = false;
        self.codex_max_output_tokens.set("");
        self.codex_chat_reasoning = CodexChatReasoningConfig::default();
        self.codex_prompt_cache_routing = PromptCacheRoutingMode::Auto;
        self.codex_model_catalog.clear();
        self.codex_local_routing_enabled = false;
        self.codex_local_routing_field_idx = 0;
        self.codex_model_catalog_idx = 0;
        self.codex_model_catalog_field = CodexModelCatalogField::Model;
    }
}
