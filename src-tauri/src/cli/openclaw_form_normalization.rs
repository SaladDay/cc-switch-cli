use std::collections::HashMap;

use serde_json::Value;

use crate::openclaw_config::{OpenClawAgentsDefaults, OpenClawDefaultModel, OpenClawToolsConfig};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OpenClawAgentsFormLike {
    pub primary_model: String,
    pub fallbacks: Vec<String>,
    pub workspace: String,
    pub timeout: String,
    pub timeout_seconds_seed: Option<Value>,
    pub context_tokens: String,
    pub context_tokens_seed: Option<Value>,
    pub max_concurrent: String,
    pub max_concurrent_seed: Option<Value>,
    pub model_catalog: Option<
        std::collections::HashMap<String, crate::openclaw_config::OpenClawModelCatalogEntry>,
    >,
    pub defaults_extra: HashMap<String, Value>,
    pub model_extra: HashMap<String, Value>,
    pub has_legacy_timeout: bool,
}

impl OpenClawAgentsFormLike {
    pub(crate) fn from_snapshot(defaults: Option<&OpenClawAgentsDefaults>) -> Self {
        let defaults = defaults.cloned().unwrap_or_default();
        let model = defaults.model.unwrap_or(OpenClawDefaultModel {
            primary: String::new(),
            fallbacks: Vec::new(),
            extra: HashMap::new(),
        });
        let mut defaults_extra = defaults.extra;
        let timeout_seconds_seed = defaults_extra.remove("timeoutSeconds");
        let legacy_timeout = defaults_extra.remove("timeout");
        let has_legacy_timeout = legacy_timeout.is_some();
        let context_tokens_seed = defaults_extra.remove("contextTokens");
        let max_concurrent_seed = defaults_extra.remove("maxConcurrent");

        let workspace = string_value(defaults_extra.remove("workspace"));
        let timeout = legacy_timeout
            .clone()
            .map(|value| string_value(Some(value)))
            .unwrap_or_else(|| numeric_value(timeout_seconds_seed.clone()));
        let context_tokens = numeric_value(context_tokens_seed.clone());
        let max_concurrent = numeric_value(max_concurrent_seed.clone());

        Self {
            primary_model: model.primary,
            fallbacks: model.fallbacks,
            workspace,
            timeout,
            timeout_seconds_seed,
            context_tokens,
            context_tokens_seed,
            max_concurrent,
            max_concurrent_seed,
            model_catalog: defaults.models,
            defaults_extra,
            model_extra: model.extra,
            has_legacy_timeout,
        }
    }

    pub(crate) fn to_config(&self) -> OpenClawAgentsDefaults {
        let mut extra = self.defaults_extra.clone();
        update_string_field(&mut extra, "workspace", &self.workspace);
        update_timeout_seconds_field(
            &mut extra,
            &self.timeout,
            self.has_legacy_timeout,
            self.timeout_seconds_seed.as_ref(),
        );
        extra.remove("timeout");
        update_number_field(
            &mut extra,
            "contextTokens",
            &self.context_tokens,
            self.context_tokens_seed.as_ref(),
        );
        update_number_field(
            &mut extra,
            "maxConcurrent",
            &self.max_concurrent,
            self.max_concurrent_seed.as_ref(),
        );

        let fallbacks = self
            .fallbacks
            .iter()
            .filter_map(|value| {
                let trimmed = value.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
            .collect::<Vec<_>>();
        let primary_model = self.primary_model.trim().to_string();
        let model =
            (!primary_model.is_empty() || !fallbacks.is_empty() || !self.model_extra.is_empty())
                .then(|| OpenClawDefaultModel {
                    primary: primary_model,
                    fallbacks,
                    extra: self.model_extra.clone(),
                });

        OpenClawAgentsDefaults {
            model,
            models: self.model_catalog.clone(),
            extra,
        }
    }

    pub(crate) fn has_unmigratable_legacy_timeout(&self) -> bool {
        self.has_legacy_timeout
            && !self.timeout.trim().is_empty()
            && parse_number(self.timeout.trim()).is_none()
    }
}

pub(crate) fn normalize_tools_config(tools: &OpenClawToolsConfig) -> OpenClawToolsConfig {
    OpenClawToolsConfig {
        profile: tools.profile.clone(),
        allow: normalize_rule_list(&tools.allow),
        deny: normalize_rule_list(&tools.deny),
        extra: tools.extra.clone(),
    }
}

fn normalize_rule_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .filter_map(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .collect()
}

pub(crate) fn string_value(value: Option<Value>) -> String {
    match value {
        Some(Value::String(value)) => value,
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

pub(crate) fn numeric_value(value: Option<Value>) -> String {
    match value {
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::String(value)) => value,
        _ => String::new(),
    }
}

pub(crate) fn update_string_field(extra: &mut HashMap<String, Value>, key: &str, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        extra.remove(key);
    } else {
        extra.insert(key.to_string(), Value::String(trimmed.to_string()));
    }
}

pub(crate) fn update_number_field(
    extra: &mut HashMap<String, Value>,
    key: &str,
    value: &str,
    seed: Option<&Value>,
) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        if should_preserve_non_string_numeric_seed(seed) {
            extra.insert(key.to_string(), seed.cloned().expect("seed exists"));
            return;
        }
        extra.remove(key);
        return;
    }

    let parsed = parse_number(trimmed);

    if let Some(number) = parsed {
        extra.insert(key.to_string(), Value::Number(number));
    } else {
        extra.insert(key.to_string(), Value::String(trimmed.to_string()));
    }
}

pub(crate) fn update_timeout_seconds_field(
    extra: &mut HashMap<String, Value>,
    value: &str,
    has_legacy_timeout: bool,
    timeout_seconds_seed: Option<&Value>,
) {
    let trimmed = value.trim();
    if let Some(number) = parse_number(trimmed) {
        extra.insert("timeoutSeconds".to_string(), Value::Number(number));
        return;
    }

    if trimmed.is_empty() && has_legacy_timeout {
        if let Some(seed) = timeout_seconds_seed {
            extra.insert("timeoutSeconds".to_string(), seed.clone());
            return;
        }
    }

    if trimmed.is_empty() {
        if should_preserve_non_string_numeric_seed(timeout_seconds_seed) {
            extra.insert(
                "timeoutSeconds".to_string(),
                timeout_seconds_seed.cloned().expect("seed exists"),
            );
            return;
        }
        extra.remove("timeoutSeconds");
    } else {
        extra.insert(
            "timeoutSeconds".to_string(),
            Value::String(trimmed.to_string()),
        );
    }
}

pub(crate) fn parse_number(value: &str) -> Option<serde_json::Number> {
    value
        .parse::<i64>()
        .ok()
        .map(serde_json::Number::from)
        .or_else(|| value.parse::<u64>().ok().map(serde_json::Number::from))
        .or_else(|| {
            value
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
        })
}

pub(crate) fn should_preserve_non_string_numeric_seed(seed: Option<&Value>) -> bool {
    matches!(seed, Some(value) if !value.is_number() && !value.is_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_tools_config_trims_and_drops_empty_rules() {
        let tools = OpenClawToolsConfig {
            profile: Some("coding".to_string()),
            allow: vec![
                " Read ".to_string(),
                "".to_string(),
                "  ".to_string(),
                "Bash(ls*)".to_string(),
            ],
            deny: vec![" Write ".to_string(), "\t".to_string()],
            extra: HashMap::new(),
        };

        let normalized = normalize_tools_config(&tools);

        assert_eq!(normalized.allow, vec!["Read", "Bash(ls*)"]);
        assert_eq!(normalized.deny, vec!["Write"]);
    }

    #[test]
    fn agents_form_like_migrates_legacy_timeout_and_preserves_unknown_fields() {
        let defaults = OpenClawAgentsDefaults {
            model: Some(OpenClawDefaultModel {
                primary: " provider/model ".to_string(),
                fallbacks: vec![" fallback/one ".to_string(), " ".to_string()],
                extra: HashMap::from([("temperature".to_string(), json!(0.2))]),
            }),
            models: None,
            extra: HashMap::from([
                ("workspace".to_string(), json!(" ./work ")),
                ("timeout".to_string(), json!(42)),
                ("contextTokens".to_string(), json!("4096")),
                ("custom".to_string(), json!(true)),
            ]),
        };

        let config = OpenClawAgentsFormLike::from_snapshot(Some(&defaults)).to_config();

        let model = config.model.expect("model should be preserved");
        assert_eq!(model.primary, "provider/model");
        assert_eq!(model.fallbacks, vec!["fallback/one"]);
        assert_eq!(model.extra.get("temperature"), Some(&json!(0.2)));
        assert_eq!(config.extra.get("workspace"), Some(&json!("./work")));
        assert_eq!(config.extra.get("timeoutSeconds"), Some(&json!(42)));
        assert_eq!(config.extra.get("contextTokens"), Some(&json!(4096)));
        assert_eq!(config.extra.get("custom"), Some(&json!(true)));
        assert!(!config.extra.contains_key("timeout"));
    }

    #[test]
    fn agents_form_like_preserves_non_string_runtime_seed_when_empty() {
        let defaults = OpenClawAgentsDefaults {
            model: None,
            models: None,
            extra: HashMap::from([("contextTokens".to_string(), json!(false))]),
        };

        let config = OpenClawAgentsFormLike::from_snapshot(Some(&defaults)).to_config();

        assert_eq!(config.extra.get("contextTokens"), Some(&json!(false)));
    }

    #[test]
    fn agents_form_like_detects_unmigratable_legacy_timeout() {
        let defaults = OpenClawAgentsDefaults {
            model: None,
            models: None,
            extra: HashMap::from([("timeout".to_string(), json!("manual"))]),
        };

        let form = OpenClawAgentsFormLike::from_snapshot(Some(&defaults));

        assert!(form.has_unmigratable_legacy_timeout());
    }
}
