use serde_json::{json, Map, Value};

pub(crate) const CLAUDE_OPUS_MODEL: &str = "claude-opus-5-5";
pub(crate) const CLAUDE_OPUS_NAME: &str = "Claude Opus 5.5";
pub(crate) const CLAUDE_SONNET_MODEL: &str = "claude-sonnet-5-5";
pub(crate) const CLAUDE_SONNET_NAME: &str = "Claude Sonnet 5.5";
pub(crate) const CLAUDE_HAIKU_MODEL: &str = "claude-haiku-5-5";
pub(crate) const CLAUDE_HAIKU_NAME: &str = "Claude Haiku 5.5";

pub(crate) const CODEX_DEFAULT_MODEL: &str = "gpt-6.1-sol";
pub(crate) const CODEX_DEFAULT_MODEL_NAME: &str = "GPT-6.1 Sol";
pub(crate) const CODEX_OAUTH_FAST_MODEL: &str = "gpt-6-luna";
// Codex 订阅后端把 GPT 系窗口封在 272K（超过 272K 输入按 2 倍计价），
// 与上游 cc-switch 的 GPT_CONTEXT_WINDOW_CAP 保持同值。
pub(crate) const CODEX_OAUTH_CONTEXT_TOKENS: &str = "272000";

pub(crate) const GEMINI_DEFAULT_MODEL: &str = "gemini-3.8-flash";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SponsorModelFamily {
    Claude,
    RunApiClaude,
    Gpt,
}

impl SponsorModelFamily {
    pub(crate) const fn primary_model(self) -> &'static str {
        match self {
            Self::Claude => CLAUDE_OPUS_MODEL,
            Self::RunApiClaude => CLAUDE_SONNET_MODEL,
            Self::Gpt => CODEX_DEFAULT_MODEL,
        }
    }

    pub(crate) const fn primary_model_name(self) -> &'static str {
        match self {
            Self::Claude => CLAUDE_OPUS_NAME,
            Self::RunApiClaude => CLAUDE_SONNET_NAME,
            Self::Gpt => CODEX_DEFAULT_MODEL_NAME,
        }
    }

    pub(crate) const fn primary_context_window(self) -> &'static str {
        match self {
            Self::Claude | Self::RunApiClaude => "1000000",
            Self::Gpt => "272000",
        }
    }
}

pub(crate) fn sponsor_model_family(preset_id: &str) -> Option<SponsorModelFamily> {
    match preset_id {
        "packycode" | "aicodemirror" | "cubence" | "openmodel" => Some(SponsorModelFamily::Claude),
        "runapi" => Some(SponsorModelFamily::RunApiClaude),
        "qiniu" | "fenno" => Some(SponsorModelFamily::Gpt),
        _ => None,
    }
}

pub(crate) fn codex_oauth_claude_env() -> Value {
    json!({
        "ANTHROPIC_BASE_URL": "https://chatgpt.com/backend-api/codex",
        "ANTHROPIC_MODEL": CODEX_DEFAULT_MODEL,
        "ANTHROPIC_DEFAULT_HAIKU_MODEL": CODEX_OAUTH_FAST_MODEL,
        "ANTHROPIC_DEFAULT_SONNET_MODEL": CODEX_DEFAULT_MODEL,
        "ANTHROPIC_DEFAULT_OPUS_MODEL": CODEX_DEFAULT_MODEL,
        "CLAUDE_CODE_MAX_CONTEXT_TOKENS": CODEX_OAUTH_CONTEXT_TOKENS,
        "CLAUDE_CODE_AUTO_COMPACT_WINDOW": CODEX_OAUTH_CONTEXT_TOKENS,
    })
}

pub(crate) fn sponsor_opencode_settings(
    provider_name: &str,
    base_url: &str,
    family: SponsorModelFamily,
) -> Value {
    let mut models = Map::new();
    match family {
        SponsorModelFamily::Claude => {
            insert_named_model(&mut models, CLAUDE_SONNET_MODEL, CLAUDE_SONNET_NAME);
            insert_named_model(&mut models, CLAUDE_OPUS_MODEL, CLAUDE_OPUS_NAME);
        }
        SponsorModelFamily::RunApiClaude => {
            insert_named_model(&mut models, CLAUDE_SONNET_MODEL, CLAUDE_SONNET_NAME);
            insert_named_model(&mut models, CLAUDE_OPUS_MODEL, CLAUDE_OPUS_NAME);
            insert_named_model(&mut models, CLAUDE_HAIKU_MODEL, CLAUDE_HAIKU_NAME);
        }
        SponsorModelFamily::Gpt => {
            insert_named_model(&mut models, CODEX_DEFAULT_MODEL, CODEX_DEFAULT_MODEL_NAME);
        }
    }

    json!({
        "npm": match family {
            SponsorModelFamily::Claude | SponsorModelFamily::RunApiClaude => {
                "@ai-sdk/anthropic"
            }
            SponsorModelFamily::Gpt => "@ai-sdk/openai-compatible",
        },
        "name": provider_name,
        "options": {
            "baseURL": base_url,
            "setCacheKey": true,
        },
        "models": models,
    })
}

pub(crate) fn sponsor_hermes_models(family: SponsorModelFamily) -> Vec<Value> {
    match family {
        SponsorModelFamily::Claude => vec![
            named_model(CLAUDE_OPUS_MODEL, CLAUDE_OPUS_NAME),
            named_model(CLAUDE_SONNET_MODEL, CLAUDE_SONNET_NAME),
            named_model(CLAUDE_HAIKU_MODEL, CLAUDE_HAIKU_NAME),
        ],
        // Hermes derives model.default from the first entry. Upstream keeps a
        // separate suggested default for RunAPI, so put Sonnet first here to
        // preserve the same effective switch behavior.
        SponsorModelFamily::RunApiClaude => vec![
            named_model(CLAUDE_SONNET_MODEL, CLAUDE_SONNET_NAME),
            named_model(CLAUDE_OPUS_MODEL, CLAUDE_OPUS_NAME),
            named_model(CLAUDE_HAIKU_MODEL, CLAUDE_HAIKU_NAME),
        ],
        SponsorModelFamily::Gpt => {
            vec![named_model(CODEX_DEFAULT_MODEL, CODEX_DEFAULT_MODEL_NAME)]
        }
    }
}

pub(crate) fn sponsor_openclaw_models(family: SponsorModelFamily) -> Vec<Value> {
    match family {
        SponsorModelFamily::Claude => vec![
            json!({
                "id": CLAUDE_OPUS_MODEL,
                "name": CLAUDE_OPUS_NAME,
                "reasoning": true,
                "input": ["text", "image"],
                "contextWindow": 1000000,
                "maxTokens": 128000,
                "cost": { "input": 4, "output": 20, "cacheRead": 0.2, "cacheWrite": 5 },
            }),
            json!({
                "id": CLAUDE_SONNET_MODEL,
                "name": CLAUDE_SONNET_NAME,
                "reasoning": true,
                "input": ["text", "image"],
                "contextWindow": 1000000,
                "maxTokens": 128000,
                "cost": { "input": 2, "output": 10, "cacheRead": 0.1, "cacheWrite": 2.5 },
            }),
        ],
        // OpenClaw also derives the no-argument default from the first entry.
        // Upstream's RunAPI preset explicitly suggests Sonnet as primary.
        SponsorModelFamily::RunApiClaude => vec![
            json!({
                "id": CLAUDE_SONNET_MODEL,
                "name": CLAUDE_SONNET_NAME,
                "reasoning": true,
                "input": ["text", "image"],
                "contextWindow": 1000000,
                "maxTokens": 128000,
                "cost": { "input": 2, "output": 10, "cacheRead": 0.1, "cacheWrite": 2.5 },
            }),
            json!({
                "id": CLAUDE_OPUS_MODEL,
                "name": CLAUDE_OPUS_NAME,
                "reasoning": true,
                "input": ["text", "image"],
                "contextWindow": 1000000,
                "maxTokens": 128000,
                "cost": { "input": 4, "output": 20, "cacheRead": 0.2, "cacheWrite": 5 },
            }),
            json!({
                "id": CLAUDE_HAIKU_MODEL,
                "name": CLAUDE_HAIKU_NAME,
                "reasoning": true,
                "input": ["text", "image"],
                "contextWindow": 1000000,
                "maxTokens": 128000,
                "cost": { "input": 0.1, "output": 0.5, "cacheRead": 0.01, "cacheWrite": 0.125 },
            }),
        ],
        SponsorModelFamily::Gpt => vec![json!({
            "id": CODEX_DEFAULT_MODEL,
            "name": CODEX_DEFAULT_MODEL_NAME,
            "contextWindow": 272000,
        })],
    }
}

fn insert_named_model(models: &mut Map<String, Value>, id: &str, name: &str) {
    models.insert(id.to_string(), json!({ "name": name }));
}

fn named_model(id: &str, name: &str) -> Value {
    json!({
        "id": id,
        "name": name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_default_model_ids_are_pinned() {
        assert_eq!(CLAUDE_OPUS_MODEL, "claude-opus-5-5");
        assert_eq!(CLAUDE_SONNET_MODEL, "claude-sonnet-5-5");
        assert_eq!(CLAUDE_HAIKU_MODEL, "claude-haiku-5-5");
        assert_eq!(CODEX_DEFAULT_MODEL, "gpt-6.1-sol");
        assert_eq!(CODEX_OAUTH_FAST_MODEL, "gpt-6-luna");
        assert_eq!(GEMINI_DEFAULT_MODEL, "gemini-3.8-flash");
    }

    #[test]
    fn runapi_additive_apps_keep_sonnet_as_effective_default() {
        assert_eq!(
            sponsor_hermes_models(SponsorModelFamily::RunApiClaude)[0]["id"],
            CLAUDE_SONNET_MODEL
        );
        assert_eq!(
            sponsor_openclaw_models(SponsorModelFamily::RunApiClaude)[0]["id"],
            CLAUDE_SONNET_MODEL
        );
    }

    #[test]
    fn sponsor_model_families_cover_every_additive_preset() {
        assert_eq!(
            sponsor_model_family("packycode"),
            Some(SponsorModelFamily::Claude)
        );
        assert_eq!(
            sponsor_model_family("aicodemirror"),
            Some(SponsorModelFamily::Claude)
        );
        assert_eq!(
            sponsor_model_family("cubence"),
            Some(SponsorModelFamily::Claude)
        );
        assert_eq!(
            sponsor_model_family("openmodel"),
            Some(SponsorModelFamily::Claude)
        );
        assert_eq!(
            sponsor_model_family("runapi"),
            Some(SponsorModelFamily::RunApiClaude)
        );
        assert_eq!(sponsor_model_family("qiniu"), Some(SponsorModelFamily::Gpt));
        assert_eq!(sponsor_model_family("fenno"), Some(SponsorModelFamily::Gpt));
    }
}
