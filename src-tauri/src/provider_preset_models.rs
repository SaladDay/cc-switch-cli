use serde_json::{json, Map, Value};

pub(crate) const CLAUDE_OPUS_MODEL: &str = "claude-opus-5";
pub(crate) const CLAUDE_OPUS_NAME: &str = "Claude Opus 5";
pub(crate) const CLAUDE_SONNET_MODEL: &str = "claude-sonnet-5";
pub(crate) const CLAUDE_SONNET_NAME: &str = "Claude Sonnet 5";
pub(crate) const CLAUDE_HAIKU_MODEL: &str = "claude-haiku-4-5";
pub(crate) const CLAUDE_HAIKU_DATED_MODEL: &str = "claude-haiku-4-5-20251001";
pub(crate) const CLAUDE_HAIKU_NAME: &str = "Claude Haiku 4.5";

pub(crate) const CODEX_DEFAULT_MODEL: &str = "gpt-5.6-sol";
pub(crate) const CODEX_DEFAULT_MODEL_NAME: &str = "GPT-5.6 Sol";
pub(crate) const CODEX_OAUTH_FAST_MODEL: &str = "gpt-5.6-luna";
pub(crate) const CODEX_OAUTH_CONTEXT_TOKENS: &str = "372000";

pub(crate) const GEMINI_DEFAULT_MODEL: &str = "gemini-3.6-flash";

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
            Self::Gpt => "400000",
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
            named_model(CLAUDE_HAIKU_DATED_MODEL, CLAUDE_HAIKU_NAME),
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
                "contextWindow": 1000000,
                "cost": {
                    "input": 5,
                    "output": 25,
                },
            }),
            json!({
                "id": CLAUDE_SONNET_MODEL,
                "name": CLAUDE_SONNET_NAME,
                "contextWindow": 1000000,
                "cost": {
                    "input": 3,
                    "output": 15,
                },
            }),
        ],
        // OpenClaw also derives the no-argument default from the first entry.
        // Upstream's RunAPI preset explicitly suggests Sonnet as primary.
        SponsorModelFamily::RunApiClaude => vec![
            json!({
                "id": CLAUDE_SONNET_MODEL,
                "name": CLAUDE_SONNET_NAME,
                "contextWindow": 1000000,
            }),
            json!({
                "id": CLAUDE_OPUS_MODEL,
                "name": CLAUDE_OPUS_NAME,
                "contextWindow": 1000000,
            }),
            json!({
                "id": CLAUDE_HAIKU_MODEL,
                "name": CLAUDE_HAIKU_NAME,
                "contextWindow": 200000,
            }),
        ],
        SponsorModelFamily::Gpt => vec![json!({
            "id": CODEX_DEFAULT_MODEL,
            "name": CODEX_DEFAULT_MODEL_NAME,
            "contextWindow": 400000,
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
        assert_eq!(CLAUDE_OPUS_MODEL, "claude-opus-5");
        assert_eq!(CLAUDE_SONNET_MODEL, "claude-sonnet-5");
        assert_eq!(CODEX_DEFAULT_MODEL, "gpt-5.6-sol");
        assert_eq!(GEMINI_DEFAULT_MODEL, "gemini-3.6-flash");
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
