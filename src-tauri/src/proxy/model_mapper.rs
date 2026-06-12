use crate::provider::Provider;
use serde_json::Value;

const ONE_M_CONTEXT_MARKER: &str = "[1m]";

pub struct ModelMapping {
    pub haiku_model: Option<String>,
    pub sonnet_model: Option<String>,
    pub opus_model: Option<String>,
    pub default_model: Option<String>,
}

impl ModelMapping {
    pub fn from_provider(provider: &Provider) -> Self {
        let env = provider.settings_config.get("env");

        Self {
            haiku_model: env
                .and_then(|value| value.get("ANTHROPIC_DEFAULT_HAIKU_MODEL"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(String::from),
            sonnet_model: env
                .and_then(|value| value.get("ANTHROPIC_DEFAULT_SONNET_MODEL"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(String::from),
            opus_model: env
                .and_then(|value| value.get("ANTHROPIC_DEFAULT_OPUS_MODEL"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(String::from),
            default_model: env
                .and_then(|value| value.get("ANTHROPIC_MODEL"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(String::from),
        }
    }

    pub fn has_mapping(&self) -> bool {
        self.haiku_model.is_some()
            || self.sonnet_model.is_some()
            || self.opus_model.is_some()
            || self.default_model.is_some()
    }

    pub fn map_model(&self, original_model: &str) -> String {
        let model_lower = original_model.to_lowercase();

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

        if let Some(model) = &self.default_model {
            return model.clone();
        }

        original_model.to_string()
    }

    pub fn map_explicit_role_model(&self, original_model: &str) -> Option<String> {
        let model_lower = original_model.to_lowercase();

        if model_lower.contains("haiku") {
            return self.haiku_model.clone();
        }
        if model_lower.contains("opus") {
            return self.opus_model.clone();
        }
        if model_lower.contains("sonnet") {
            return self.sonnet_model.clone();
        }

        None
    }
}

pub fn provider_has_explicit_role_mapping(provider: &Provider, original_model: &str) -> bool {
    let Some(mapped) =
        ModelMapping::from_provider(provider).map_explicit_role_model(original_model)
    else {
        return false;
    };

    mapped.trim() != original_model.trim()
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
    fn detects_explicit_role_mapping_without_using_default_model() {
        let mut provider = provider_with_mapping("deepseek-v4-pro [1M]");
        provider.settings_config["env"]["ANTHROPIC_MODEL"] = json!("default-model");

        assert!(provider_has_explicit_role_mapping(
            &provider,
            "claude-sonnet-4-6[1M]"
        ));
        assert!(!provider_has_explicit_role_mapping(
            &provider,
            "some-custom-model"
        ));
    }
}
