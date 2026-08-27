use crate::app_config::AppType;
use crate::provider::{ClaudeApiKeyField, CodexChatReasoningConfig, Provider};
use crate::provider_preset_builtin::{builtin_provider_preset_value, BuiltinProviderPresetId};
use crate::provider_preset_models::{
    codex_oauth_claude_env, sponsor_hermes_models, sponsor_model_family, sponsor_openclaw_models,
    sponsor_opencode_settings, SponsorModelFamily, CODEX_DEFAULT_MODEL, CODEX_OAUTH_FAST_MODEL,
    GEMINI_DEFAULT_MODEL,
};
use crate::provider_preset_pi::{
    PiProviderPreset, PI_BUILTIN_PROVIDER_PRESETS, PI_SPONSOR_PROVIDER_PRESETS,
};
use crate::provider_preset_sponsors::{
    sponsor_provider_preset, sponsor_provider_presets_for_app, SponsorProviderPreset,
};
use serde_json::json;

use super::provider_state_loading::populate_form_from_provider;
use super::{
    ClaudeApiFormat, CodexModelCatalogField, CodexWireApi, FormMode, GeminiAuthType,
    PromptCacheRoutingMode, ProviderAddFormState, HERMES_DEFAULT_API_MODE,
    OPENCLAW_DEFAULT_API_PROTOCOL,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderTemplateId {
    Custom,
    ClaudeOfficial,
    CodexOAuth,
    OpenAiOfficial,
    Builtin(BuiltinProviderPresetId),
    GoogleOAuth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProviderTemplateDef {
    id: ProviderTemplateId,
    label: &'static str,
}

/// Section grouping used by the provider template picker overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTemplateSection {
    BuiltIn,
    Sponsors,
}

/// One display row of the provider template picker.
///
/// Headers are never selectable. Every item carries the flat template index,
/// so `apply_template` stays correct even though the picker groups templates
/// in a different order than [`ProviderAddFormState::template_labels`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTemplateRow {
    Header(ProviderTemplateSection),
    Item {
        flat_idx: usize,
        label: &'static str,
        section: ProviderTemplateSection,
    },
}

impl ProviderTemplateRow {
    pub fn flat_idx(&self) -> Option<usize> {
        match self {
            ProviderTemplateRow::Item { flat_idx, .. } => Some(*flat_idx),
            ProviderTemplateRow::Header(_) => None,
        }
    }
}

/// Sponsor chip labels carry a leading `"* "` marker so they stand out in the
/// flat chip row. The picker renders them under an explicit Sponsors header,
/// so the marker is redundant there.
fn strip_sponsor_chip_marker(label: &'static str) -> &'static str {
    label.strip_prefix("* ").unwrap_or(label)
}

/// Display-row index of `flat_idx`, when the picker still contains it.
pub fn provider_template_row_for_flat_idx(
    rows: &[ProviderTemplateRow],
    flat_idx: usize,
) -> Option<usize> {
    rows.iter().position(|row| row.flat_idx() == Some(flat_idx))
}

/// First selectable flat index; used to recover from a stale selection.
pub fn provider_template_first_flat_idx(rows: &[ProviderTemplateRow]) -> Option<usize> {
    rows.iter().find_map(ProviderTemplateRow::flat_idx)
}

/// Neighbouring selectable flat index, skipping non-selectable section
/// headers. Returns `None` at either end so callers keep the current row.
pub fn provider_template_step_flat_idx(
    rows: &[ProviderTemplateRow],
    flat_idx: usize,
    forward: bool,
) -> Option<usize> {
    let current = provider_template_row_for_flat_idx(rows, flat_idx)?;
    if forward {
        rows.get(current.saturating_add(1)..)?
            .iter()
            .find_map(ProviderTemplateRow::flat_idx)
    } else {
        rows.get(..current)?
            .iter()
            .rev()
            .find_map(ProviderTemplateRow::flat_idx)
    }
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

static PROVIDER_TEMPLATE_DEFS_CLAUDE_AFTER_SPONSORS: [ProviderTemplateDef; 9] = [
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::DeepSeek),
        label: "DeepSeek",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::ZhipuGlm),
        label: "Zhipu GLM",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::ZhipuGlmEn),
        label: "Zhipu GLM en",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::ModelScope),
        label: "ModelScope",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::MiniMax),
        label: "MiniMax",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::XiaomiMimo),
        label: "Xiaomi MiMo",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::XiaomiMimoTokenPlan),
        label: "Xiaomi MiMo Token Plan (China)",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::OpenCodeGo),
        label: "OpenCode Go",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::OpenRouter),
        label: "OpenRouter",
    },
];

static PROVIDER_TEMPLATE_DEFS_CODEX_AFTER_SPONSORS: [ProviderTemplateDef; 9] = [
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::DeepSeek),
        label: "DeepSeek",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::ZhipuGlm),
        label: "Zhipu GLM",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::ZhipuGlmEn),
        label: "Zhipu GLM en",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::ModelScope),
        label: "ModelScope",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::MiniMax),
        label: "MiniMax",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::XiaomiMimo),
        label: "Xiaomi MiMo",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::XiaomiMimoTokenPlan),
        label: "Xiaomi MiMo Token Plan (China)",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::OpenCodeGo),
        label: "OpenCode Go",
    },
    ProviderTemplateDef {
        id: ProviderTemplateId::Builtin(BuiltinProviderPresetId::OpenRouter),
        label: "OpenRouter",
    },
];

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
        AppType::Pi => &PROVIDER_TEMPLATE_DEFS_OPENCLAW,
    }
}

pub(super) fn provider_sponsor_presets(app_type: &AppType) -> &'static [SponsorProviderPreset] {
    sponsor_provider_presets_for_app(app_type)
}

fn pi_provider_preset_for_flat_idx(flat_idx: usize) -> Option<&'static PiProviderPreset> {
    let preset_idx = flat_idx.checked_sub(1)?;
    PI_BUILTIN_PROVIDER_PRESETS.get(preset_idx).or_else(|| {
        PI_SPONSOR_PROVIDER_PRESETS
            .get(preset_idx.saturating_sub(PI_BUILTIN_PROVIDER_PRESETS.len()))
    })
}

pub(super) fn provider_after_sponsor_template_defs(
    app_type: &AppType,
) -> &'static [ProviderTemplateDef] {
    match app_type {
        AppType::Claude => &PROVIDER_TEMPLATE_DEFS_CLAUDE_AFTER_SPONSORS,
        AppType::Codex => &PROVIDER_TEMPLATE_DEFS_CODEX_AFTER_SPONSORS,
        AppType::Gemini | AppType::OpenCode | AppType::Hermes | AppType::OpenClaw | AppType::Pi => {
            &[]
        }
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
        if matches!(self.app_type, AppType::Pi) {
            return 1 + PI_BUILTIN_PROVIDER_PRESETS.len() + PI_SPONSOR_PROVIDER_PRESETS.len();
        }
        provider_builtin_template_defs(&self.app_type).len()
            + provider_sponsor_presets(&self.app_type).len()
            + provider_after_sponsor_template_defs(&self.app_type).len()
    }

    /// Flat template labels, indexed the same way as [`Self::apply_template`].
    ///
    /// Sponsor labels drop the `"* "` chip marker here, matching the picker
    /// overlay: the collapsed template row shows one label at a time, so the
    /// marker no longer separates sponsors from built-ins the way it did in
    /// the old chip row (which the MCP form and the CLI chooser still use).
    pub fn template_labels(&self) -> Vec<&'static str> {
        if matches!(self.app_type, AppType::Pi) {
            let mut labels = Vec::with_capacity(self.template_count());
            labels.push("Custom");
            labels.extend(
                PI_BUILTIN_PROVIDER_PRESETS
                    .iter()
                    .map(|preset| preset.label),
            );
            labels.extend(
                PI_SPONSOR_PROVIDER_PRESETS
                    .iter()
                    .map(|preset| preset.label),
            );
            return labels;
        }
        let mut labels = provider_builtin_template_defs(&self.app_type)
            .iter()
            .map(|def| def.label)
            .collect::<Vec<_>>();
        labels.extend(
            provider_sponsor_presets(&self.app_type)
                .iter()
                .map(|preset| strip_sponsor_chip_marker(preset.chip_label)),
        );
        labels.extend(
            provider_after_sponsor_template_defs(&self.app_type)
                .iter()
                .map(|def| def.label),
        );
        labels
    }

    /// Display rows for the template picker overlay.
    ///
    /// Built-in templates and the after-sponsor built-ins (e.g. Codex's
    /// DeepSeek) share one section; sponsor presets get their own. Section
    /// headers are only emitted when the app actually has sponsor presets,
    /// so single-section apps stay a plain list.
    pub fn template_picker_rows(&self) -> Vec<ProviderTemplateRow> {
        if matches!(self.app_type, AppType::Pi) {
            let mut rows = Vec::with_capacity(self.template_count() + 2);
            rows.push(ProviderTemplateRow::Header(
                ProviderTemplateSection::BuiltIn,
            ));
            rows.push(ProviderTemplateRow::Item {
                flat_idx: 0,
                label: "Custom",
                section: ProviderTemplateSection::BuiltIn,
            });
            rows.extend(
                PI_BUILTIN_PROVIDER_PRESETS
                    .iter()
                    .enumerate()
                    .map(|(offset, preset)| ProviderTemplateRow::Item {
                        flat_idx: 1 + offset,
                        label: preset.label,
                        section: ProviderTemplateSection::BuiltIn,
                    }),
            );
            rows.push(ProviderTemplateRow::Header(
                ProviderTemplateSection::Sponsors,
            ));
            rows.extend(
                PI_SPONSOR_PROVIDER_PRESETS
                    .iter()
                    .enumerate()
                    .map(|(offset, preset)| ProviderTemplateRow::Item {
                        flat_idx: 1 + PI_BUILTIN_PROVIDER_PRESETS.len() + offset,
                        label: preset.label,
                        section: ProviderTemplateSection::Sponsors,
                    }),
            );
            return rows;
        }
        let builtin_defs = provider_builtin_template_defs(&self.app_type);
        let sponsor_presets = provider_sponsor_presets(&self.app_type);
        let after_sponsor_defs = provider_after_sponsor_template_defs(&self.app_type);

        let grouped = !sponsor_presets.is_empty();
        // Every template plus the two section headers.
        let mut rows = Vec::with_capacity(self.template_count().saturating_add(2));

        if grouped {
            rows.push(ProviderTemplateRow::Header(
                ProviderTemplateSection::BuiltIn,
            ));
        }
        for (idx, def) in builtin_defs.iter().enumerate() {
            rows.push(ProviderTemplateRow::Item {
                flat_idx: idx,
                label: def.label,
                section: ProviderTemplateSection::BuiltIn,
            });
        }
        for (offset, def) in after_sponsor_defs.iter().enumerate() {
            rows.push(ProviderTemplateRow::Item {
                flat_idx: builtin_defs.len() + sponsor_presets.len() + offset,
                label: def.label,
                section: ProviderTemplateSection::BuiltIn,
            });
        }

        if grouped {
            rows.push(ProviderTemplateRow::Header(
                ProviderTemplateSection::Sponsors,
            ));
            for (offset, preset) in sponsor_presets.iter().enumerate() {
                rows.push(ProviderTemplateRow::Item {
                    flat_idx: builtin_defs.len() + offset,
                    label: strip_sponsor_chip_marker(preset.chip_label),
                    section: ProviderTemplateSection::Sponsors,
                });
            }
        }

        rows
    }

    pub fn apply_template(&mut self, idx: usize, existing_ids: &[String]) {
        let builtin_defs = provider_builtin_template_defs(&self.app_type);
        let sponsor_presets = provider_sponsor_presets(&self.app_type);
        let after_sponsor_defs = provider_after_sponsor_template_defs(&self.app_type);
        let total_templates = self.template_count();
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

        if matches!(self.app_type, AppType::Pi) && idx > 0 {
            if let Some(preset) = pi_provider_preset_for_flat_idx(idx) {
                self.apply_pi_provider_preset(preset);
            }
        } else if idx >= builtin_defs.len() && idx < builtin_defs.len() + sponsor_presets.len() {
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
                ProviderTemplateId::Builtin(preset) => {
                    self.apply_builtin_provider_preset(preset);
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

    fn apply_builtin_provider_preset(&mut self, preset: BuiltinProviderPresetId) {
        let app_type = self.app_type.clone();
        let Some(provider_value) = builtin_provider_preset_value(&app_type, preset) else {
            return;
        };
        let Ok(provider) = serde_json::from_value::<Provider>(provider_value.clone()) else {
            return;
        };

        match app_type {
            AppType::Claude => self.reset_claude_template_state(),
            AppType::Codex => self.reset_codex_template_state(),
            _ => return,
        }
        self.extra = provider_value;
        self.name.set(&provider.name);
        self.website_url
            .set(provider.website_url.as_deref().unwrap_or_default());
        self.notes.set("");
        populate_form_from_provider(self, &app_type, &provider);
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
            AppType::Pi => {}
        }

        if matches!(self.app_type, AppType::Codex) {
            self.codex_local_routing_enabled = !self.codex_model_catalog.is_empty();
        }
    }

    fn apply_pi_provider_preset(&mut self, preset: &PiProviderPreset) {
        let settings = preset.settings_config();
        let sponsor = preset.sponsor_id.and_then(sponsor_provider_preset);
        let mut extra = json!({
            "category": preset.category,
            "icon": preset.icon,
            "settingsConfig": settings.clone(),
        });

        if let Some(icon_color) = preset.icon_color {
            extra["iconColor"] = json!(icon_color);
        }
        if let Some(sponsor) = sponsor {
            extra["meta"] = json!({
                "isPartner": true,
                "partnerPromotionKey": sponsor.partner_promotion_key,
            });
        } else if let Some(partner_promotion_key) = preset.partner_promotion_key {
            extra["meta"] = json!({
                "partnerPromotionKey": partner_promotion_key,
            });
        }

        self.extra = extra;
        self.id.set(preset.provider_key);
        self.id_is_manual = true;
        self.name.set(preset.label);
        self.website_url
            .set(sponsor.map_or(preset.website_url, |item| item.website_url));
        self.notes.set("");
        self.openclaw_user_agent = false;

        self.opencode_api_key.set(
            settings
                .get("apiKey")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
        );
        self.opencode_base_url.set(
            settings
                .get("baseUrl")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
        );
        self.opencode_npm_package.set(
            settings
                .get("api")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
        );
        self.openclaw_models = settings
            .get("models")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();

        self.opencode_model_id.set("");
        self.opencode_model_name.set("");
        self.opencode_model_context_limit.set("");
        self.opencode_model_output_limit.set("");
        self.opencode_model_original_id = None;
        if let Some(model) = self.openclaw_models.first() {
            if let Some(id) = model.get("id").and_then(serde_json::Value::as_str) {
                self.opencode_model_id.set(id);
                self.opencode_model_original_id = Some(id.to_string());
            }
            if let Some(name) = model.get("name").and_then(serde_json::Value::as_str) {
                self.opencode_model_name.set(name);
            }
            if let Some(context_window) = model
                .get("contextWindow")
                .and_then(serde_json::Value::as_u64)
            {
                self.opencode_model_context_limit
                    .set(context_window.to_string());
            }
            if let Some(max_tokens) = model.get("maxTokens").and_then(serde_json::Value::as_u64) {
                self.opencode_model_output_limit.set(max_tokens.to_string());
            }
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
