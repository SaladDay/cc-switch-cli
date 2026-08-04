use serde_json::{json, Value};

use super::types::OptimizerConfig;

pub fn optimize(body: &mut Value, config: &OptimizerConfig) {
    if !config.thinking_optimizer {
        return;
    }

    let model = match body.get("model").and_then(|m| m.as_str()) {
        Some(model) => model.to_lowercase(),
        None => return,
    };

    if model.contains("haiku") {
        return;
    }

    if uses_adaptive_thinking(&model) {
        body["thinking"] = json!({"type": "adaptive"});
        body["output_config"] = json!({"effort": "max"});
        append_beta(body, "context-1m-2025-08-07");
        return;
    }

    let max_tokens = body
        .get("max_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(16_384);
    let budget_target = max_tokens.saturating_sub(1);
    let thinking_type = body
        .get("thinking")
        .and_then(|thinking| thinking.get("type"))
        .and_then(|value| value.as_str());

    match thinking_type {
        None | Some("disabled") => {
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": budget_target,
            });
        }
        Some("enabled") => {
            let current_budget = body
                .get("thinking")
                .and_then(|thinking| thinking.get("budget_tokens"))
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            if current_budget < budget_target {
                body["thinking"]["budget_tokens"] = json!(budget_target);
            }
        }
        Some(_) => {}
    }

    append_beta(body, "interleaved-thinking-2025-05-14");
}

pub(crate) fn uses_adaptive_thinking(model: &str) -> bool {
    let normalized = normalize_model_name(model);
    [
        "fable-5",
        "mythos-5",
        "mythos-preview",
        "sonnet-5",
        "opus-4-8",
        "opus-4-7",
        "opus-4-6",
        "sonnet-4-6",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

/// Models where omitting `thinking` still leaves adaptive thinking enabled.
pub(crate) fn adaptive_thinking_is_default(model: &str) -> bool {
    let normalized = normalize_model_name(model);
    ["fable-5", "mythos-5", "mythos-preview", "sonnet-5"]
        .iter()
        .any(|needle| normalized.contains(needle))
}

/// Models that reject `thinking: {"type":"disabled"}`.
pub(crate) fn thinking_cannot_be_disabled(model: &str) -> bool {
    let normalized = normalize_model_name(model);
    ["fable-5", "mythos-5"]
        .iter()
        .any(|needle| normalized.contains(needle))
}

fn normalize_model_name(model: &str) -> String {
    model.trim().to_ascii_lowercase().replace(['.', '_'], "-")
}

fn append_beta(body: &mut Value, beta: &str) {
    match body.get("anthropic_beta") {
        Some(Value::Array(existing))
            if existing.iter().any(|value| value.as_str() == Some(beta)) => {}
        Some(Value::Array(_)) => body["anthropic_beta"]
            .as_array_mut()
            .unwrap()
            .push(json!(beta)),
        _ => body["anthropic_beta"] = json!([beta]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_config() -> OptimizerConfig {
        OptimizerConfig {
            enabled: true,
            thinking_optimizer: true,
            cache_injection: true,
            cache_ttl: "1h".to_string(),
        }
    }

    #[test]
    fn adaptive_models_use_adaptive_thinking() {
        let mut body = json!({
            "model": "anthropic.claude-opus-4-6-20250514-v1:0",
            "max_tokens": 128,
        });

        optimize(&mut body, &enabled_config());

        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "max");
        assert!(body["anthropic_beta"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "context-1m-2025-08-07"));
    }

    #[test]
    fn legacy_models_enable_thinking_budget() {
        let mut body = json!({
            "model": "anthropic.claude-sonnet-4-5-20250514-v1:0",
            "max_tokens": 32,
        });

        optimize(&mut body, &enabled_config());

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 31);
        assert!(body["anthropic_beta"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "interleaved-thinking-2025-05-14"));
    }
}
