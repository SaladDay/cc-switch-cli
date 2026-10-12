use crate::{
    claude_model_config::{ClaudeModelRole, CLAUDE_DEFAULT_MODEL_ENV_KEY},
    provider::Provider,
};
use serde_json::Value;
use std::collections::BTreeMap;

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

/// Route mode exact mapping: replaces the model only on an exact table hit,
/// leaves it untouched otherwise
///
/// Called before the existing role-keyword mapping (`apply_model_mapping`);
/// the mapped result then flows through normal `[1m]` handling and upstream
/// override logic
///
/// When the raw request model misses the table, the lookup retries on the
/// `[1m]`-stripped name: mapping keys are seeded bare by the TUI picker, and
/// a `name[1m]` request should still hit the bare `name` entry instead of
/// silently no-oping. Exact hits (including keys that carry the marker
/// themselves) keep priority.
pub fn apply_route_model_mapping(mut body: Value, mappings: &BTreeMap<String, String>) -> Value {
    if mappings.is_empty() {
        return body;
    }

    if let Some(model) = body.get("model").and_then(Value::as_str) {
        let actual = mappings
            .get(model)
            .or_else(|| mappings.get(strip_one_m_suffix_for_upstream(model)));
        if let Some(actual) = actual {
            body["model"] = serde_json::json!(actual);
        }
    }
    body
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

    #[test]
    fn route_mapping_rewrites_only_matched_model() {
        let mut mappings = BTreeMap::new();
        mappings.insert("m-a".to_string(), "m-b".to_string());

        let hit = apply_route_model_mapping(json!({"model": "m-a"}), &mappings);
        assert_eq!(hit["model"], "m-b");

        let miss = apply_route_model_mapping(json!({"model": "m-x"}), &mappings);
        assert_eq!(miss["model"], "m-x");

        let no_model = apply_route_model_mapping(json!({"prompt": "hi"}), &mappings);
        assert_eq!(no_model.get("model"), None);
    }

    #[test]
    fn route_mapping_empty_table_is_noop() {
        let mappings = BTreeMap::new();
        let body = json!({"model": "m-a"});

        let result = apply_route_model_mapping(body, &mappings);

        assert_eq!(result["model"], "m-a");
    }

    #[test]
    fn route_mapping_hits_bare_key_for_one_m_suffixed_request() {
        // Picker-seeded keys are bare; a `name[1m]` request must still hit
        // the bare entry instead of silently no-oping
        let mut mappings = BTreeMap::new();
        mappings.insert("m-a".to_string(), "m-b".to_string());

        let hit = apply_route_model_mapping(json!({"model": "m-a[1m]"}), &mappings);
        assert_eq!(hit["model"], "m-b");

        // Exact keys that carry the marker themselves keep priority
        let mut marker_key = BTreeMap::new();
        marker_key.insert("m-a[1m]".to_string(), "m-marker".to_string());
        marker_key.insert("m-a".to_string(), "m-bare".to_string());
        let exact = apply_route_model_mapping(json!({"model": "m-a[1m]"}), &marker_key);
        assert_eq!(exact["model"], "m-marker");

        // Unrelated models stay untouched
        let miss = apply_route_model_mapping(json!({"model": "m-x[1m]"}), &mappings);
        assert_eq!(miss["model"], "m-x[1m]");
    }

    #[test]
    fn route_mapping_applies_before_role_mapping() {
        // The route mapping output must contain the role keyword ("sonnet")
        // to keep triggering the existing role mapping
        let mut mappings = BTreeMap::new();
        mappings.insert("claude-sonnet-4-6".to_string(), "m-sonnet-a".to_string());
        let provider = provider_with_mapping("sonnet-mapped");
        let body = json!({"model": "claude-sonnet-4-6"});

        let routed = apply_route_model_mapping(body, &mappings);
        assert_eq!(routed["model"], "m-sonnet-a");

        let (result, _, _) = apply_model_mapping(routed, &provider);

        assert_eq!(result["model"], "sonnet-mapped");
    }
}
