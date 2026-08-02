use crate::{
    claude_model_config::{ClaudeModelRole, CLAUDE_DEFAULT_MODEL_ENV_KEY},
    provider::Provider,
};
use serde_json::Value;

const ONE_M_CONTEXT_MARKER: &str = "[1m]";

pub struct ModelMapping {
    pub haiku_model: Option<String>,
    pub sonnet_model: Option<String>,
    pub opus_model: Option<String>,
    pub fable_model: Option<String>,
    pub subagent_model: Option<String>,
    pub default_model: Option<String>,
}

impl ModelMapping {
    pub fn from_provider(provider: &Provider) -> Self {
        let env = provider.settings_config.get("env");
        let model_for = |role: ClaudeModelRole| {
            env.and_then(|value| value.get(role.model_env_key()))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(String::from)
        };

        Self {
            haiku_model: model_for(ClaudeModelRole::Haiku),
            sonnet_model: model_for(ClaudeModelRole::Sonnet),
            opus_model: model_for(ClaudeModelRole::Opus),
            fable_model: model_for(ClaudeModelRole::Fable),
            subagent_model: model_for(ClaudeModelRole::Subagent),
            default_model: env
                .and_then(|value| value.get(CLAUDE_DEFAULT_MODEL_ENV_KEY))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(String::from),
        }
    }

    pub fn has_mapping(&self) -> bool {
        self.haiku_model.is_some()
            || self.sonnet_model.is_some()
            || self.opus_model.is_some()
            || self.fable_model.is_some()
            || self.subagent_model.is_some()
            || self.default_model.is_some()
    }

    pub fn map_model(&self, original_model: &str) -> String {
        let model_lower = original_model.to_lowercase();

        if model_lower.contains("fable") {
            if let Some(model) = &self.fable_model {
                return model.clone();
            }
            if let Some(model) = &self.opus_model {
                return model.clone();
            }
        }
        if model_lower.contains("haiku") {
            if let Some(model) = &self.haiku_model {
                return model.clone();
            }
        }
        if model_lower.contains("opus") {
            if let Some(model) = &self.opus_model {
                return model.clone();
            }
        }
        if model_lower.contains("sonnet") {
            if let Some(model) = &self.sonnet_model {
                return model.clone();
            }
        }

        if let Some(model) = &self.subagent_model {
            if strip_one_m_suffix_for_upstream(original_model)
                == strip_one_m_suffix_for_upstream(model)
            {
                return original_model.to_string();
            }
        }

        if let Some(model) = &self.default_model {
            return model.clone();
        }

        original_model.to_string()
    }
}

pub fn apply_model_mapping(
    mut body: Value,
    provider: &Provider,
) -> (Value, Option<String>, Option<String>) {
    let mapping = ModelMapping::from_provider(provider);

    if !mapping.has_mapping() {
        let original = body.get("model").and_then(Value::as_str).map(String::from);
        return (body, original, None);
    }

    let original_model = body.get("model").and_then(Value::as_str).map(String::from);

    if let Some(original) = &original_model {
        let mapped = mapping.map_model(original);

        if mapped != *original {
            body["model"] = serde_json::json!(mapped);
            return (body, Some(original.clone()), Some(mapped));
        }
    }

    (body, original_model, None)
}

pub fn strip_one_m_suffix_for_upstream(model: &str) -> &str {
    let trimmed = model.trim_end();
    let marker = ONE_M_CONTEXT_MARKER.as_bytes();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= marker.len()
        && bytes[bytes.len() - marker.len()..].eq_ignore_ascii_case(marker)
    {
        return trimmed[..trimmed.len() - marker.len()].trim_end();
    }
    model
}

pub fn strip_one_m_suffix_for_upstream_from_body(mut body: Value) -> Value {
    let Some(model) = body.get("model").and_then(Value::as_str) else {
        return body;
    };

    let stripped = strip_one_m_suffix_for_upstream(model);
    if stripped != model {
        body["model"] = serde_json::json!(stripped);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn provider_with_mapping(mapped_model: &str) -> Provider {
        Provider {
            id: "test".to_string(),
            name: "Test".to_string(),
            settings_config: json!({
                "env": {
                    "ANTHROPIC_DEFAULT_SONNET_MODEL": mapped_model
                }
            }),
            website_url: None,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    fn provider_with_env(env: Value) -> Provider {
        Provider {
            id: "test".to_string(),
            name: "Test".to_string(),
            settings_config: json!({ "env": env }),
            website_url: None,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    #[test]
    fn thinking_does_not_use_legacy_reasoning_model_mapping() {
        let mut provider = provider_with_mapping("sonnet-mapped");
        provider.settings_config["env"]["ANTHROPIC_REASONING_MODEL"] = json!("reasoning-mapped");
        let body = json!({
            "model": "claude-sonnet-4-6",
            "thinking": {"type": "enabled"}
        });

        let (result, _, mapped) = apply_model_mapping(body, &provider);

        assert_eq!(result["model"], "sonnet-mapped");
        assert_eq!(mapped, Some("sonnet-mapped".to_string()));
    }

    #[test]
    fn maps_fable_to_explicit_fable_model() {
        let provider = provider_with_env(json!({
            "ANTHROPIC_MODEL": "default-model",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "opus-model",
            "ANTHROPIC_DEFAULT_FABLE_MODEL": "fable-model"
        }));

        let (result, _, mapped) =
            apply_model_mapping(json!({"model": "claude-fable-5[1M]"}), &provider);

        assert_eq!(result["model"], "fable-model");
        assert_eq!(mapped, Some("fable-model".to_string()));
    }

    #[test]
    fn fable_falls_back_to_opus_then_default() {
        let opus_provider = provider_with_env(json!({
            "ANTHROPIC_MODEL": "default-model",
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "opus-model"
        }));
        let default_provider = provider_with_env(json!({
            "ANTHROPIC_MODEL": "default-model"
        }));

        let (opus_result, _, _) =
            apply_model_mapping(json!({"model": "claude-fable-5"}), &opus_provider);
        let (default_result, _, _) =
            apply_model_mapping(json!({"model": "claude-fable-5"}), &default_provider);

        assert_eq!(opus_result["model"], "opus-model");
        assert_eq!(default_result["model"], "default-model");
    }

    #[test]
    fn preserves_subagent_model_before_default_fallback() {
        let provider = provider_with_env(json!({
            "ANTHROPIC_MODEL": "default-model",
            "CLAUDE_CODE_SUBAGENT_MODEL": "gpt-5.4-mini"
        }));

        for model in ["gpt-5.4-mini", "gpt-5.4-mini[1M]"] {
            let (result, original, mapped) =
                apply_model_mapping(json!({"model": model}), &provider);
            assert_eq!(result["model"], model);
            assert_eq!(original.as_deref(), Some(model));
            assert!(mapped.is_none());
        }
    }

    #[test]
    fn strips_one_m_suffix_before_upstream() {
        let body = json!({"model": "deepseek-v4-pro[1M]"});
        let result = strip_one_m_suffix_for_upstream_from_body(body);
        assert_eq!(result["model"], "deepseek-v4-pro");
    }

    #[test]
    fn strips_one_m_suffix_after_mapping() {
        let provider = provider_with_mapping("deepseek-v4-pro [1M]");
        let body = json!({"model": "claude-sonnet-4-6"});

        let (mapped, _, _) = apply_model_mapping(body, &provider);
        let result = strip_one_m_suffix_for_upstream_from_body(mapped);

        assert_eq!(result["model"], "deepseek-v4-pro");
    }

    #[test]
    fn keeps_model_without_one_m_suffix() {
        let body = json!({"model": "deepseek-v4-pro"});
        let result = strip_one_m_suffix_for_upstream_from_body(body);
        assert_eq!(result["model"], "deepseek-v4-pro");
    }
}
