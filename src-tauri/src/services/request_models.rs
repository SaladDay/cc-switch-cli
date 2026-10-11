//! Request-model candidates for route-mode mapping defaults.
//!
//! The request models an agent will actually send are already known: they
//! come from the route app's active provider config. Extraction here is
//! purely local (no network) so the TUI can offer a picker on the mapping
//! editor's request field and seed passthrough mappings on member add.

use crate::app_config::AppType;
use crate::claude_model_config::{
    split_claude_one_m_marker, ClaudeModelRole, CLAUDE_DEFAULT_MODEL_ENV_KEY,
};
use crate::provider::Provider;

/// Models the agent behind `app_type` may send as request models, read from
/// the given provider's config. Deterministic order (Claude: `ANTHROPIC_MODEL`
/// first, then model-role order) with duplicates dropped, first occurrence
/// wins.
pub(crate) fn request_model_candidates(provider: &Provider, app_type: &AppType) -> Vec<String> {
    let mut candidates = Vec::new();
    for model in extract(provider, app_type) {
        if !candidates.contains(&model) {
            candidates.push(model);
        }
    }
    candidates
}

fn extract(provider: &Provider, app_type: &AppType) -> Vec<String> {
    match app_type {
        AppType::Claude => claude_candidates(provider),
        AppType::Codex => codex_candidates(provider),
        AppType::Gemini => gemini_candidates(provider),
        _ => Vec::new(),
    }
}

/// Claude reads its model roles from `settings_config["env"]`: the default
/// model first, then the per-role overrides. `[1m]` context markers are
/// stripped so the picker lists bare model ids.
fn claude_candidates(provider: &Provider) -> Vec<String> {
    let Some(env) = provider
        .settings_config
        .get("env")
        .and_then(|value| value.as_object())
    else {
        return Vec::new();
    };
    let mut models = Vec::new();
    let mut push_env = |key: &str| {
        if let Some(value) = env.get(key).and_then(|value| value.as_str()) {
            let (model, _) = split_claude_one_m_marker(value);
            let model = model.trim().to_string();
            if !model.is_empty() {
                models.push(model);
            }
        }
    };
    push_env(CLAUDE_DEFAULT_MODEL_ENV_KEY);
    for role in ClaudeModelRole::ALL {
        push_env(role.model_env_key());
    }
    models
}

/// Codex keeps its model in the top-level `model` key of the generated TOML;
/// cc-switch writes no profiles table, so that single value is the whole pool.
fn codex_candidates(provider: &Provider) -> Vec<String> {
    provider
        .settings_config
        .get("config")
        .and_then(|value| value.as_str())
        .and_then(crate::codex_config::codex_top_level_model)
        .into_iter()
        .collect()
}

/// Gemini keeps its env map in `settings_config["env"]`; only `GEMINI_MODEL`
/// names the request model. A missing or non-string `GEMINI_MODEL` yields no
/// candidates.
fn gemini_candidates(provider: &Provider) -> Vec<String> {
    crate::gemini_config::json_to_env(&provider.settings_config)
        .ok()
        .and_then(|env| env.get("GEMINI_MODEL").cloned())
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_provider(settings: serde_json::Value) -> Provider {
        serde_json::from_value(json!({
            "id": "p1",
            "name": "test",
            "settingsConfig": settings,
        }))
        .expect("provider fixture should deserialize")
    }

    #[test]
    fn claude_lists_default_then_roles_and_strips_one_m_marker() {
        let provider = make_provider(json!({
            "env": {
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-4-6",
                "ANTHROPIC_MODEL": "claude-opus-4-8[1m]",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "claude-haiku-4-5",
                "ANTHROPIC_DEFAULT_FABLE_MODEL": "",
            }
        }));
        assert_eq!(
            request_model_candidates(&provider, &AppType::Claude),
            vec!["claude-opus-4-8", "claude-haiku-4-5", "claude-sonnet-4-6",]
        );
    }

    #[test]
    fn claude_dedupes_repeated_models_keeping_first_position() {
        let provider = make_provider(json!({
            "env": {
                "ANTHROPIC_MODEL": "claude-opus-4-8",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-opus-4-8",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-4-6",
            }
        }));
        assert_eq!(
            request_model_candidates(&provider, &AppType::Claude),
            vec!["claude-opus-4-8", "claude-sonnet-4-6"]
        );
    }

    #[test]
    fn claude_without_env_yields_nothing() {
        let provider = make_provider(json!({}));
        assert!(request_model_candidates(&provider, &AppType::Claude).is_empty());
    }

    #[test]
    fn codex_reads_top_level_toml_model() {
        let provider = make_provider(json!({ "config": "model = \"gpt-5.1\"\n" }));
        assert_eq!(
            request_model_candidates(&provider, &AppType::Codex),
            vec!["gpt-5.1"]
        );
    }

    #[test]
    fn codex_without_model_or_invalid_toml_yields_nothing() {
        let provider = make_provider(json!({ "config": "sandbox = true\n" }));
        assert!(request_model_candidates(&provider, &AppType::Codex).is_empty());

        let provider = make_provider(json!({ "config": "model = [\"gpt-5.1\"]" }));
        assert!(request_model_candidates(&provider, &AppType::Codex).is_empty());
    }

    #[test]
    fn gemini_reads_gemini_model_env_key() {
        let provider = make_provider(json!({
            "env": { "GEMINI_MODEL": "gemini-2.5-pro" }
        }));
        assert_eq!(
            request_model_candidates(&provider, &AppType::Gemini),
            vec!["gemini-2.5-pro"]
        );
    }

    #[test]
    fn gemini_missing_model_yields_nothing() {
        let provider = make_provider(json!({
            "env": { "GEMINI_API_KEY": "key" }
        }));
        assert!(request_model_candidates(&provider, &AppType::Gemini).is_empty());
    }

    #[test]
    fn non_route_apps_yield_nothing() {
        let provider = make_provider(json!({ "env": { "ANTHROPIC_MODEL": "claude-opus-4-8" } }));
        assert!(request_model_candidates(&provider, &AppType::OpenCode).is_empty());
        assert!(request_model_candidates(&provider, &AppType::Hermes).is_empty());
    }
}
