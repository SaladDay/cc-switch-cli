use crate::app_config::AppType;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuiltinProviderPresetId {
    DeepSeek,
    ZhipuGlm,
    ZhipuGlmEn,
    ModelScope,
    MiniMax,
    XiaomiMimo,
    XiaomiMimoTokenPlan,
    OpenCodeGo,
    OpenRouter,
}

pub(crate) fn builtin_provider_preset_value(
    app_type: &AppType,
    preset: BuiltinProviderPresetId,
) -> Option<Value> {
    match app_type {
        AppType::Claude => Some(claude_provider_preset(preset)),
        AppType::Codex => Some(codex_provider_preset(preset)),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn claude_provider(
    name: &str,
    website_url: &str,
    base_url: &str,
    api_key_field: &str,
    model: &str,
    haiku_model: &str,
    sonnet_model: &str,
    opus_model: &str,
    category: &str,
    icon: &str,
    icon_color: &str,
    env_extra: Option<Value>,
) -> Value {
    let mut env = json!({
        "ANTHROPIC_BASE_URL": base_url,
        (api_key_field): "",
        "ANTHROPIC_MODEL": model,
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": haiku_model,
        "ANTHROPIC_DEFAULT_SONNET_MODEL": sonnet_model,
        "ANTHROPIC_DEFAULT_OPUS_MODEL": opus_model,
    });
    if let (Some(env), Some(extra)) = (
        env.as_object_mut(),
        env_extra.and_then(|v| v.as_object().cloned()),
    ) {
        env.extend(extra);
    }

    let mut provider = json!({
        "id": "",
        "name": name,
        "websiteUrl": website_url,
        "category": category,
        "icon": icon,
        "iconColor": icon_color,
        "settingsConfig": { "env": env },
    });
    if api_key_field == "ANTHROPIC_API_KEY" {
        provider["meta"] = json!({ "apiKeyField": "ANTHROPIC_API_KEY" });
    }
    provider
}

fn claude_provider_preset(preset: BuiltinProviderPresetId) -> Value {
    use BuiltinProviderPresetId::*;

    match preset {
        DeepSeek => claude_provider(
            "DeepSeek",
            "https://platform.deepseek.com",
            "https://api.deepseek.com/anthropic",
            "ANTHROPIC_AUTH_TOKEN",
            "deepseek-v4-pro",
            "deepseek-v4-flash",
            "deepseek-v4-pro",
            "deepseek-v4-pro",
            "cn_official",
            "deepseek",
            "#1E88E5",
            None,
        ),
        ZhipuGlm => claude_provider(
            "Zhipu GLM",
            "https://open.bigmodel.cn",
            "https://open.bigmodel.cn/api/anthropic",
            "ANTHROPIC_AUTH_TOKEN",
            "glm-5.1",
            "glm-5.1",
            "glm-5.1",
            "glm-5.1",
            "cn_official",
            "zhipu",
            "#0F62FE",
            None,
        ),
        ZhipuGlmEn => claude_provider(
            "Zhipu GLM en",
            "https://z.ai",
            "https://api.z.ai/api/anthropic",
            "ANTHROPIC_AUTH_TOKEN",
            "glm-5.1",
            "glm-5.1",
            "glm-5.1",
            "glm-5.1",
            "cn_official",
            "zhipu",
            "#0F62FE",
            None,
        ),
        ModelScope => claude_provider(
            "ModelScope",
            "https://modelscope.cn",
            "https://api-inference.modelscope.cn",
            "ANTHROPIC_AUTH_TOKEN",
            "ZhipuAI/GLM-5.2",
            "ZhipuAI/GLM-5.2",
            "ZhipuAI/GLM-5.2",
            "ZhipuAI/GLM-5.2",
            "aggregator",
            "modelscope",
            "#624AFF",
            None,
        ),
        MiniMax => claude_provider(
            "MiniMax",
            "https://platform.minimaxi.com",
            "https://api.minimaxi.com/anthropic",
            "ANTHROPIC_AUTH_TOKEN",
            "MiniMax-M2.7",
            "MiniMax-M2.7",
            "MiniMax-M2.7",
            "MiniMax-M2.7",
            "cn_official",
            "minimax",
            "#FF6B6B",
            Some(json!({
                "API_TIMEOUT_MS": "3000000",
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": 1,
            })),
        ),
        XiaomiMimo => claude_provider(
            "Xiaomi MiMo",
            "https://platform.xiaomimimo.com",
            "https://api.xiaomimimo.com/anthropic",
            "ANTHROPIC_AUTH_TOKEN",
            "mimo-v2.5-pro",
            "mimo-v2.5-pro",
            "mimo-v2.5-pro",
            "mimo-v2.5-pro",
            "cn_official",
            "xiaomimimo",
            "#000000",
            None,
        ),
        XiaomiMimoTokenPlan => claude_provider(
            "Xiaomi MiMo Token Plan (China)",
            "https://platform.xiaomimimo.com/#/token-plan",
            "https://token-plan-cn.xiaomimimo.com/anthropic",
            "ANTHROPIC_AUTH_TOKEN",
            "mimo-v2.5-pro",
            "mimo-v2.5-pro",
            "mimo-v2.5-pro",
            "mimo-v2.5-pro",
            "cn_official",
            "xiaomimimo",
            "#000000",
            None,
        ),
        OpenCodeGo => claude_provider(
            "OpenCode Go",
            "https://opencode.ai/go",
            "https://opencode.ai/zen/go",
            "ANTHROPIC_API_KEY",
            "deepseek-v4-flash",
            "deepseek-v4-flash",
            "deepseek-v4-flash",
            "deepseek-v4-flash",
            "third_party",
            "opencode",
            "#211E1E",
            None,
        ),
        OpenRouter => claude_provider(
            "OpenRouter",
            "https://openrouter.ai",
            "https://openrouter.ai/api",
            "ANTHROPIC_AUTH_TOKEN",
            "anthropic/claude-sonnet-5",
            "anthropic/claude-haiku-4.5",
            "anthropic/claude-sonnet-5",
            "anthropic/claude-opus-5",
            "aggregator",
            "openrouter",
            "#6566F1",
            None,
        ),
    }
}

fn codex_config(provider_name: &str, base_url: &str, model: &str) -> String {
    format!(
        r#"model_provider = "custom"
model = "{model}"
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.custom]
name = "{provider_name}"
base_url = "{base_url}"
wire_api = "responses"
requires_openai_auth = true"#
    )
}

#[allow(clippy::too_many_arguments)]
fn codex_provider(
    name: &str,
    website_url: &str,
    provider_name: &str,
    base_url: &str,
    model: &str,
    category: &str,
    icon: &str,
    icon_color: &str,
    api_format: Option<&str>,
    model_catalog: Vec<Value>,
    reasoning: Option<Value>,
) -> Value {
    let mut meta = serde_json::Map::new();
    if let Some(api_format) = api_format {
        meta.insert("apiFormat".to_string(), json!(api_format));
    }
    if let Some(reasoning) = reasoning {
        meta.insert("codexChatReasoning".to_string(), reasoning);
    }

    let mut settings = json!({
        "config": codex_config(provider_name, base_url, model),
    });
    if !model_catalog.is_empty() {
        settings["modelCatalog"] = json!({ "models": model_catalog });
    }

    let mut provider = json!({
        "id": "",
        "name": name,
        "websiteUrl": website_url,
        "category": category,
        "icon": icon,
        "iconColor": icon_color,
        "settingsConfig": settings,
    });
    if !meta.is_empty() {
        provider["meta"] = Value::Object(meta);
    }
    provider
}

fn chat_reasoning(
    thinking_param: &str,
    supports_effort: bool,
    effort_value_mode: Option<&str>,
) -> Value {
    let mut reasoning = json!({
        "supportsThinking": true,
        "supportsEffort": supports_effort,
        "thinkingParam": thinking_param,
        "effortParam": if supports_effort { "reasoning_effort" } else { "none" },
        "outputFormat": "reasoning_content",
    });
    if let Some(mode) = effort_value_mode {
        reasoning["effortValueMode"] = json!(mode);
    }
    reasoning
}

fn mimo_catalog() -> Vec<Value> {
    let base_instructions = "You are MiMo, an AI assistant developed by Xiaomi. Today's date: {date} {week}. Your knowledge cutoff date is December 2024.";
    vec![
        json!({
            "model": "mimo-v2.5-pro",
            "displayName": "MiMo V2.5 Pro",
            "contextWindow": 1_048_576,
            "inputModalities": ["text"],
            "reasoningLevels": ["none", "high"],
            "baseInstructions": base_instructions,
        }),
        json!({
            "model": "mimo-v2.5",
            "displayName": "MiMo V2.5",
            "contextWindow": 1_048_576,
            "inputModalities": ["text", "image"],
            "reasoningLevels": ["none", "high"],
            "baseInstructions": base_instructions,
        }),
    ]
}

fn codex_provider_preset(preset: BuiltinProviderPresetId) -> Value {
    use BuiltinProviderPresetId::*;

    match preset {
        DeepSeek => codex_provider(
            "DeepSeek",
            "https://platform.deepseek.com",
            "deepseek",
            "https://api.deepseek.com",
            "deepseek-v4-flash",
            "cn_official",
            "deepseek",
            "#1E88E5",
            Some("openai_responses"),
            vec![
                json!({
                    "model": "deepseek-v4-flash",
                    "displayName": "DeepSeek V4 Flash",
                    "contextWindow": 1_048_576,
                    "reasoningLevels": ["low", "high", "max"],
                }),
                json!({
                    "model": "deepseek-v4-pro",
                    "displayName": "DeepSeek V4 Pro",
                    "contextWindow": 1_048_576,
                    "reasoningLevels": ["low", "high", "max"],
                }),
            ],
            None,
        ),
        ZhipuGlm | ZhipuGlmEn => {
            let (name, website, provider_name, base_url) = if preset == ZhipuGlm {
                (
                    "Zhipu GLM",
                    "https://open.bigmodel.cn",
                    "zhipu_glm",
                    "https://open.bigmodel.cn/api/coding/paas/v4",
                )
            } else {
                (
                    "Zhipu GLM en",
                    "https://z.ai",
                    "zhipu_glm_en",
                    "https://api.z.ai/api/coding/paas/v4",
                )
            };
            codex_provider(
                name,
                website,
                provider_name,
                base_url,
                "glm-5.2",
                "cn_official",
                "zhipu",
                "#0F62FE",
                Some("openai_chat"),
                vec![json!({
                    "model": "glm-5.2",
                    "displayName": "GLM-5.2",
                    "contextWindow": 200_000,
                    "reasoningLevels": ["none", "high"],
                })],
                Some(chat_reasoning("thinking", false, None)),
            )
        }
        ModelScope => codex_provider(
            "ModelScope",
            "https://modelscope.cn",
            "modelscope",
            "https://api-inference.modelscope.cn/v1",
            "ZhipuAI/GLM-5.2",
            "aggregator",
            "modelscope",
            "#624AFF",
            Some("openai_chat"),
            vec![json!({
                "model": "ZhipuAI/GLM-5.2",
                "displayName": "ZhipuAI / GLM-5.2",
                "contextWindow": 200_000,
            })],
            Some(chat_reasoning("enable_thinking", false, None)),
        ),
        MiniMax => codex_provider(
            "MiniMax",
            "https://platform.minimaxi.com",
            "minimax",
            "https://api.minimaxi.com/v1",
            "MiniMax-M3",
            "cn_official",
            "minimax",
            "#FF6B6B",
            Some("openai_responses"),
            vec![json!({
                "model": "MiniMax-M3",
                "displayName": "MiniMax-M3",
                "contextWindow": 1_000_000,
                "reasoningLevels": ["none", "high"],
                "supportsParallelToolCalls": true,
                "inputModalities": ["text", "image"],
                "baseInstructions": "You are Codex, a coding agent based on MiniMax-M3. You and the user share the same workspace and collaborate to achieve the user's goals.",
            })],
            None,
        ),
        XiaomiMimo | XiaomiMimoTokenPlan => {
            let (name, website, provider_name, base_url) = if preset == XiaomiMimo {
                (
                    "Xiaomi MiMo",
                    "https://platform.xiaomimimo.com",
                    "xiaomi_mimo",
                    "https://api.xiaomimimo.com/v1",
                )
            } else {
                (
                    "Xiaomi MiMo Token Plan (China)",
                    "https://platform.xiaomimimo.com/#/token-plan",
                    "xiaomi_mimo_token_plan",
                    "https://token-plan-cn.xiaomimimo.com/v1",
                )
            };
            codex_provider(
                name,
                website,
                provider_name,
                base_url,
                "mimo-v2.5-pro",
                "cn_official",
                "xiaomimimo",
                "#000000",
                Some("openai_responses"),
                mimo_catalog(),
                None,
            )
        }
        OpenCodeGo => codex_provider(
            "OpenCode Go",
            "https://opencode.ai/go",
            "opencode_go",
            "https://opencode.ai/zen/go/v1",
            "glm-5.2",
            "third_party",
            "opencode",
            "#211E1E",
            Some("openai_chat"),
            vec![
                json!({ "model": "glm-5.2", "displayName": "GLM 5.2", "contextWindow": 204_800, "reasoningLevels": ["high", "max"] }),
                json!({ "model": "glm-5.1", "displayName": "GLM 5.1", "contextWindow": 204_800 }),
                json!({ "model": "kimi-k2.7-code", "displayName": "Kimi K2.7 Code", "contextWindow": 262_144 }),
                json!({ "model": "deepseek-v4-pro", "displayName": "DeepSeek V4 Pro", "contextWindow": 1_048_576, "reasoningLevels": ["high", "max"] }),
                json!({ "model": "deepseek-v4-flash", "displayName": "DeepSeek V4 Flash", "contextWindow": 1_048_576, "reasoningLevels": ["low", "high", "max"] }),
                json!({ "model": "mimo-v2.5-pro", "displayName": "MiMo V2.5 Pro", "contextWindow": 1_048_576 }),
            ],
            Some(chat_reasoning("none", true, Some("zen"))),
        ),
        OpenRouter => codex_provider(
            "OpenRouter",
            "https://openrouter.ai",
            "openrouter",
            "https://openrouter.ai/api/v1",
            "gpt-5.6-sol",
            "aggregator",
            "openrouter",
            "#6566F1",
            None,
            Vec::new(),
            None,
        ),
    }
}
