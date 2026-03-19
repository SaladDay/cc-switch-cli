use crate::app_config::AppType;
use serde_json::json;

use super::{ClaudeApiFormat, CodexWireApi, FormMode, GeminiAuthType, ProviderAddFormState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderTemplateId {
    Custom,
    ClaudeOfficial,
    OpenAiOfficial,
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
}

const SPONSOR_PROVIDER_PRESETS: [SponsorProviderPreset; 2] = [
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
    },
    SponsorProviderPreset {
        id: "rightcode",
        provider_name: "RightCode",
        chip_label: "* RightCode",
        website_url: "https://right.codes",
        register_url: "https://www.right.codes/register?aff=ccswitch-cli",
        promo_code: "",
        partner_promotion_key: "rightcode",
        claude_base_url: "https://www.right.codes/claude",
        codex_base_url: "https://right.codes/codex/v1",
        gemini_base_url: "https://www.right.codes",
    },
];

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

const PROVIDER_TEMPLATE_DEFS_OPENCODE: [ProviderTemplateDef; 1] = [ProviderTemplateDef {
    id: ProviderTemplateId::Custom,
    label: "Custom",
}];

const NO_SPONSOR_PROVIDER_PRESETS: [SponsorProviderPreset; 0] = [];

pub(super) fn provider_builtin_template_defs(app_type: &AppType) -> &'static [ProviderTemplateDef] {
    match app_type {
        AppType::Claude => &PROVIDER_TEMPLATE_DEFS_CLAUDE,
        AppType::Codex => &PROVIDER_TEMPLATE_DEFS_CODEX,
        AppType::Gemini => &PROVIDER_TEMPLATE_DEFS_GEMINI,
        AppType::OpenCode => &PROVIDER_TEMPLATE_DEFS_OPENCODE,
        AppType::OpenClaw => &PROVIDER_TEMPLATE_DEFS_OPENCODE,
    }
}

pub(super) fn provider_sponsor_presets(app_type: &AppType) -> &'static [SponsorProviderPreset] {
    match app_type {
        AppType::OpenCode | AppType::OpenClaw => &NO_SPONSOR_PROVIDER_PRESETS,
        _ => &SPONSOR_PROVIDER_PRESETS,
    }
}

impl ProviderAddFormState {
    pub fn template_count(&self) -> usize {
        provider_builtin_template_defs(&self.app_type).len()
            + provider_sponsor_presets(&self.app_type).len()
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
        labels
    }

    pub fn apply_template(&mut self, idx: usize, existing_ids: &[String]) {
        let builtin_defs = provider_builtin_template_defs(&self.app_type);
        let sponsor_presets = provider_sponsor_presets(&self.app_type);
        let total_templates = builtin_defs.len() + sponsor_presets.len();
        let idx = idx.min(total_templates.saturating_sub(1));
        self.template_idx = idx;
        self.id_is_manual = false;

        if idx >= builtin_defs.len() {
            let sponsor_idx = idx.saturating_sub(builtin_defs.len());
            if let Some(preset) = sponsor_presets.get(sponsor_idx) {
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
                    self.claude_base_url.set("");
                    self.claude_api_format = ClaudeApiFormat::Anthropic;
                    self.claude_model.set("");
                    self.claude_reasoning_model.set("");
                    self.claude_haiku_model.set("");
                    self.claude_sonnet_model.set("");
                    self.claude_opus_model.set("");
                    self.claude_model_config_touched = false;
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
            AppType::OpenCode => {}
            AppType::OpenClaw => {}
        }
    }
}
