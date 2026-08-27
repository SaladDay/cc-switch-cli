use serde_json::{json, Value};

#[derive(Debug, Clone, Copy)]
pub(crate) struct PiProviderPreset {
    pub(crate) label: &'static str,
    pub(crate) provider_key: &'static str,
    pub(crate) website_url: &'static str,
    pub(crate) category: &'static str,
    pub(crate) icon: &'static str,
    pub(crate) icon_color: Option<&'static str>,
    pub(crate) partner_promotion_key: Option<&'static str>,
    pub(crate) sponsor_id: Option<&'static str>,
    settings: fn() -> Value,
}

impl PiProviderPreset {
    pub(crate) fn settings_config(&self) -> Value {
        (self.settings)()
    }
}

pub(crate) static PI_BUILTIN_PROVIDER_PRESETS: [PiProviderPreset; 9] = [
    PiProviderPreset {
        label: "DeepSeek",
        provider_key: "cc-switch-deep-seek",
        website_url: "https://platform.deepseek.com",
        category: "cn_official",
        icon: "deepseek",
        icon_color: Some("#1E88E5"),
        partner_promotion_key: None,
        sponsor_id: None,
        settings: deepseek_settings,
    },
    PiProviderPreset {
        label: "Zhipu GLM",
        provider_key: "cc-switch-zhipu-glm",
        website_url: "https://open.bigmodel.cn",
        category: "cn_official",
        icon: "zhipu",
        icon_color: Some("#0F62FE"),
        partner_promotion_key: None,
        sponsor_id: None,
        settings: zhipu_glm_settings,
    },
    PiProviderPreset {
        label: "Zhipu GLM en",
        provider_key: "cc-switch-zhipu-glm-en",
        website_url: "https://z.ai",
        category: "cn_official",
        icon: "zhipu",
        icon_color: Some("#0F62FE"),
        partner_promotion_key: None,
        sponsor_id: None,
        settings: zhipu_glm_en_settings,
    },
    PiProviderPreset {
        label: "ModelScope",
        provider_key: "cc-switch-model-scope",
        website_url: "https://modelscope.cn",
        category: "aggregator",
        icon: "modelscope",
        icon_color: Some("#624AFF"),
        partner_promotion_key: None,
        sponsor_id: None,
        settings: modelscope_settings,
    },
    PiProviderPreset {
        label: "MiniMax",
        provider_key: "cc-switch-mini-max",
        website_url: "https://platform.minimaxi.com",
        category: "cn_official",
        icon: "minimax",
        icon_color: Some("#FF6B6B"),
        partner_promotion_key: Some("minimax_cn"),
        sponsor_id: None,
        settings: minimax_settings,
    },
    PiProviderPreset {
        label: "Xiaomi MiMo",
        provider_key: "cc-switch-xiaomi-mi-mo",
        website_url: "https://platform.xiaomimimo.com",
        category: "cn_official",
        icon: "xiaomimimo",
        icon_color: Some("#000000"),
        partner_promotion_key: None,
        sponsor_id: None,
        settings: xiaomi_mimo_settings,
    },
    PiProviderPreset {
        label: "Xiaomi MiMo Token Plan (China)",
        provider_key: "cc-switch-xiaomi-mi-mo-token-plan-china",
        website_url: "https://platform.xiaomimimo.com/#/token-plan",
        category: "cn_official",
        icon: "xiaomimimo",
        icon_color: Some("#000000"),
        partner_promotion_key: None,
        sponsor_id: None,
        settings: xiaomi_mimo_token_plan_settings,
    },
    PiProviderPreset {
        label: "OpenCode Go",
        provider_key: "cc-switch-open-code-go",
        website_url: "https://opencode.ai/go",
        category: "third_party",
        icon: "opencode",
        icon_color: Some("#211E1E"),
        partner_promotion_key: Some("opencode_go"),
        sponsor_id: None,
        settings: opencode_go_settings,
    },
    PiProviderPreset {
        label: "OpenRouter",
        provider_key: "cc-switch-open-router",
        website_url: "https://openrouter.ai",
        category: "aggregator",
        icon: "openrouter",
        icon_color: Some("#6566F1"),
        partner_promotion_key: None,
        sponsor_id: None,
        settings: openrouter_settings,
    },
];

pub(crate) static PI_SPONSOR_PROVIDER_PRESETS: [PiProviderPreset; 6] = [
    PiProviderPreset {
        label: "PackyCode",
        provider_key: "cc-switch-packy-code",
        website_url: "https://www.packyapi.ai",
        category: "third_party",
        icon: "packycode",
        icon_color: None,
        partner_promotion_key: None,
        sponsor_id: Some("packycode"),
        settings: packycode_settings,
    },
    PiProviderPreset {
        label: "AICodeMirror",
        provider_key: "cc-switch-aicode-mirror",
        website_url: "https://www.aicodemirror.ai",
        category: "third_party",
        icon: "aicodemirror",
        icon_color: Some("#000000"),
        partner_promotion_key: None,
        sponsor_id: Some("aicodemirror"),
        settings: aicodemirror_settings,
    },
    PiProviderPreset {
        label: "FennoAI",
        provider_key: "cc-switch-fenno-ai",
        website_url: "https://api.fenno.ai",
        category: "aggregator",
        icon: "fenno",
        icon_color: None,
        partner_promotion_key: None,
        sponsor_id: Some("fenno"),
        settings: fenno_settings,
    },
    PiProviderPreset {
        label: "RunAPI",
        provider_key: "cc-switch-run-api",
        website_url: "https://runapi.co",
        category: "aggregator",
        icon: "runapi",
        icon_color: None,
        partner_promotion_key: None,
        sponsor_id: Some("runapi"),
        settings: runapi_settings,
    },
    PiProviderPreset {
        label: "Qiniu",
        provider_key: "cc-switch-qiniu",
        website_url: "https://s.qiniu.com/nMvAvy",
        category: "aggregator",
        icon: "qiniu",
        icon_color: None,
        partner_promotion_key: None,
        sponsor_id: Some("qiniu"),
        settings: qiniu_settings,
    },
    PiProviderPreset {
        label: "Cubence",
        provider_key: "cc-switch-cubence",
        website_url: "https://cubence.com",
        category: "third_party",
        icon: "cubence",
        icon_color: Some("#000000"),
        partner_promotion_key: None,
        sponsor_id: Some("cubence"),
        settings: cubence_settings,
    },
];

fn claude_sonnet(id: &str) -> Value {
    json!({
        "name": "Claude Sonnet 5",
        "reasoning": true,
        "input": ["text", "image"],
        "contextWindow": 1_000_000,
        "maxTokens": 128_000,
        "id": id,
        "thinkingLevelMap": { "xhigh": "xhigh", "max": "max" },
        "compat": { "forceAdaptiveThinking": true },
    })
}

fn claude_opus(id: &str) -> Value {
    json!({
        "name": "Claude Opus 5",
        "reasoning": true,
        "input": ["text", "image"],
        "contextWindow": 1_000_000,
        "maxTokens": 128_000,
        "id": id,
        "thinkingLevelMap": { "xhigh": "xhigh", "max": "max" },
        "compat": { "forceAdaptiveThinking": true },
    })
}

fn gpt_5_6_sol() -> Value {
    json!({
        "name": "GPT-5.6 Sol",
        "reasoning": true,
        "input": ["text", "image"],
        "contextWindow": 272_000,
        "maxTokens": 128_000,
        "id": "gpt-5.6-sol",
        "thinkingLevelMap": {},
    })
}

fn glm_5_1() -> Value {
    json!({
        "name": "GLM-5.1",
        "reasoning": true,
        "input": ["text"],
        "contextWindow": 200_000,
        "maxTokens": 131_072,
        "id": "glm-5.1",
        "thinkingLevelMap": {},
    })
}

fn anthropic_settings(name: &str, base_url: &str) -> Value {
    json!({
        "name": name,
        "baseUrl": base_url,
        "api": "anthropic-messages",
        "apiKey": "",
        "models": [
            claude_sonnet("claude-sonnet-5"),
            claude_opus("claude-opus-5"),
        ],
    })
}

fn openai_gpt_settings(name: &str, base_url: &str) -> Value {
    json!({
        "name": name,
        "baseUrl": base_url,
        "api": "openai-completions",
        "apiKey": "",
        "models": [gpt_5_6_sol()],
    })
}

fn packycode_settings() -> Value {
    anthropic_settings("PackyCode", "https://www.packyapi.ai")
}

fn aicodemirror_settings() -> Value {
    anthropic_settings("AICodeMirror", "https://api.aicodemirror.ai/api/claudecode")
}

fn fenno_settings() -> Value {
    openai_gpt_settings("FennoAI", "https://api.fenno.ai/v1")
}

fn runapi_settings() -> Value {
    let mut settings = anthropic_settings("RunAPI", "https://runapi.co");
    settings["models"]
        .as_array_mut()
        .expect("Pi RunAPI models are an array")
        .push(json!({
            "name": "Claude Haiku 4.5 (latest)",
            "reasoning": true,
            "input": ["text", "image"],
            "contextWindow": 200_000,
            "maxTokens": 64_000,
            "id": "claude-haiku-4-5",
            "thinkingLevelMap": {},
        }));
    settings
}

fn qiniu_settings() -> Value {
    openai_gpt_settings("Qiniu", "https://api.qnaigc.com/v1")
}

fn cubence_settings() -> Value {
    anthropic_settings("Cubence", "https://api.cubence.com")
}

fn deepseek_model(name: &str, id: &str) -> Value {
    json!({
        "name": name,
        "reasoning": true,
        "input": ["text"],
        "contextWindow": 1_000_000,
        "maxTokens": 384_000,
        "id": id,
        "thinkingLevelMap": {
            "minimal": null,
            "low": null,
            "medium": null,
            "high": "high",
            "max": "max",
        },
    })
}

fn deepseek_settings() -> Value {
    json!({
        "name": "DeepSeek",
        "baseUrl": "https://api.deepseek.com/v1",
        "api": "openai-completions",
        "apiKey": "",
        "models": [
            deepseek_model("DeepSeek V4 Pro", "deepseek-v4-pro"),
            deepseek_model("DeepSeek V4 Flash", "deepseek-v4-flash"),
        ],
    })
}

fn zhipu_glm_settings() -> Value {
    json!({
        "name": "Zhipu GLM",
        "baseUrl": "https://open.bigmodel.cn/api/coding/paas/v4",
        "api": "openai-completions",
        "apiKey": "",
        "models": [glm_5_1()],
    })
}

fn zhipu_glm_en_settings() -> Value {
    json!({
        "name": "Zhipu GLM en",
        "baseUrl": "https://api.z.ai/api/coding/paas/v4",
        "api": "openai-completions",
        "apiKey": "",
        "models": [glm_5_1()],
    })
}

fn modelscope_settings() -> Value {
    json!({
        "name": "ModelScope",
        "baseUrl": "https://api-inference.modelscope.cn/v1",
        "api": "openai-completions",
        "apiKey": "",
        "models": [{
            "name": "GLM-5.2",
            "reasoning": true,
            "input": ["text"],
            "contextWindow": 1_000_000,
            "maxTokens": 131_072,
            "id": "ZhipuAI/GLM-5.2",
            "thinkingLevelMap": {},
        }],
    })
}

fn minimax_settings() -> Value {
    json!({
        "name": "MiniMax",
        "baseUrl": "https://api.minimaxi.com/v1",
        "api": "openai-completions",
        "apiKey": "",
        "models": [{
            "name": "MiniMax-M2.7",
            "reasoning": true,
            "input": ["text"],
            "contextWindow": 204_800,
            "maxTokens": 131_072,
            "id": "MiniMax-M2.7",
            "thinkingLevelMap": {},
        }],
    })
}

fn xiaomi_model(name: &str, id: &str, input: &[&str], compat: bool) -> Value {
    let mut model = json!({
        "name": name,
        "reasoning": true,
        "input": input,
        "contextWindow": 1_048_576,
        "maxTokens": 131_072,
        "id": id,
        "thinkingLevelMap": {},
    });
    if compat {
        model["compat"] = json!({
            "requiresReasoningContentOnAssistantMessages": true,
            "thinkingFormat": "deepseek",
        });
    }
    model
}

fn xiaomi_mimo_settings() -> Value {
    json!({
        "name": "Xiaomi MiMo",
        "baseUrl": "https://api.xiaomimimo.com/v1",
        "api": "openai-completions",
        "apiKey": "",
        "models": [
            xiaomi_model("MiMo-V2.5-Pro", "mimo-v2.5-pro", &["text"], true),
            xiaomi_model("MiMo-V2.5", "mimo-v2.5", &["text", "image"], true),
        ],
    })
}

fn xiaomi_mimo_token_plan_settings() -> Value {
    json!({
        "name": "Xiaomi MiMo Token Plan (China)",
        "baseUrl": "https://token-plan-cn.xiaomimimo.com/v1",
        "api": "openai-completions",
        "apiKey": "",
        "models": [
            xiaomi_model("MiMo-V2.5-Pro", "mimo-v2.5-pro", &["text"], false),
            xiaomi_model("MiMo-V2.5", "mimo-v2.5", &["text", "image"], false),
        ],
    })
}

fn opencode_compat(extra: Value) -> Value {
    let mut compat = json!({
        "supportsStore": false,
        "supportsDeveloperRole": false,
        "maxTokensField": "max_tokens",
    });
    if let (Some(compat), Some(extra)) = (compat.as_object_mut(), extra.as_object()) {
        compat.extend(extra.clone());
    }
    compat
}

fn opencode_go_settings() -> Value {
    let thinking_levels = json!({
        "minimal": null,
        "low": null,
        "medium": null,
        "high": "high",
        "max": "max",
    });
    json!({
        "name": "OpenCode Go",
        "baseUrl": "https://opencode.ai/zen/go/v1",
        "api": "openai-completions",
        "apiKey": "",
        "models": [
            {
                "name": "GLM 5.2",
                "reasoning": true,
                "input": ["text"],
                "contextWindow": 1_000_000,
                "maxTokens": 131_072,
                "id": "glm-5.2",
                "compat": opencode_compat(json!({})),
                "thinkingLevelMap": {
                    "off": null,
                    "minimal": null,
                    "low": null,
                    "medium": null,
                    "high": "high",
                    "xhigh": null,
                    "max": "max",
                },
            },
            {
                "name": "Kimi K2.7 Code",
                "reasoning": true,
                "input": ["text", "image"],
                "contextWindow": 262_144,
                "maxTokens": 262_144,
                "id": "kimi-k2.7-code",
                "compat": opencode_compat(json!({})),
                "thinkingLevelMap": {},
            },
            {
                "name": "DeepSeek V4 Pro",
                "reasoning": true,
                "input": ["text"],
                "contextWindow": 1_000_000,
                "maxTokens": 384_000,
                "id": "deepseek-v4-pro",
                "compat": opencode_compat(json!({
                    "requiresReasoningContentOnAssistantMessages": true,
                    "thinkingFormat": "deepseek",
                })),
                "thinkingLevelMap": thinking_levels.clone(),
            },
            {
                "name": "DeepSeek V4 Flash",
                "reasoning": true,
                "input": ["text"],
                "contextWindow": 1_000_000,
                "maxTokens": 384_000,
                "id": "deepseek-v4-flash",
                "compat": opencode_compat(json!({
                    "requiresReasoningContentOnAssistantMessages": true,
                    "thinkingFormat": "deepseek",
                })),
                "thinkingLevelMap": thinking_levels,
            },
            {
                "name": "MiMo-V2.5-Pro",
                "reasoning": true,
                "input": ["text"],
                "contextWindow": 1_048_576,
                "maxTokens": 131_072,
                "id": "mimo-v2.5-pro",
                "compat": opencode_compat(json!({})),
                "thinkingLevelMap": {},
            },
        ],
    })
}

fn openrouter_settings() -> Value {
    json!({
        "name": "OpenRouter",
        "baseUrl": "https://openrouter.ai/api",
        "api": "anthropic-messages",
        "apiKey": "",
        "models": [
            claude_sonnet("anthropic/claude-sonnet-5"),
            claude_opus("anthropic/claude-opus-5"),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn pi_preset_scope_is_the_selected_upstream_subset() {
        assert_eq!(
            PI_BUILTIN_PROVIDER_PRESETS
                .iter()
                .map(|preset| preset.label)
                .collect::<Vec<_>>(),
            [
                "DeepSeek",
                "Zhipu GLM",
                "Zhipu GLM en",
                "ModelScope",
                "MiniMax",
                "Xiaomi MiMo",
                "Xiaomi MiMo Token Plan (China)",
                "OpenCode Go",
                "OpenRouter",
            ]
        );
        assert_eq!(
            PI_SPONSOR_PROVIDER_PRESETS
                .iter()
                .map(|preset| preset.sponsor_id.expect("sponsor id"))
                .collect::<Vec<_>>(),
            [
                "packycode",
                "aicodemirror",
                "fenno",
                "runapi",
                "qiniu",
                "cubence",
            ]
        );
    }

    #[test]
    fn pi_preset_keys_are_unique_and_native_settings_are_complete() {
        let presets = PI_BUILTIN_PROVIDER_PRESETS
            .iter()
            .chain(PI_SPONSOR_PROVIDER_PRESETS.iter());
        let mut keys = HashSet::new();
        for preset in presets {
            assert!(keys.insert(preset.provider_key), "duplicate provider key");
            let settings = preset.settings_config();
            assert_eq!(settings["name"], preset.label);
            assert!(settings["baseUrl"].is_string());
            assert!(settings["api"].is_string());
            assert_eq!(settings["apiKey"], "");
            assert!(settings["models"]
                .as_array()
                .is_some_and(|items| !items.is_empty()));
        }
    }
}
