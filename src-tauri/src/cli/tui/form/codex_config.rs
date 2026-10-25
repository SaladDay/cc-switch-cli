use super::CodexWireApi;

#[derive(Debug, Default)]
pub(crate) struct ParsedCodexConfigSnippet {
    pub(crate) base_url: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) wire_api: Option<CodexWireApi>,
    pub(crate) requires_openai_auth: Option<bool>,
    pub(crate) env_key: Option<String>,
}

pub(crate) fn parse_codex_config_snippet(cfg: &str) -> ParsedCodexConfigSnippet {
    let mut out = ParsedCodexConfigSnippet::default();
    let table: toml::Table = match toml::from_str(cfg.trim()) {
        Ok(table) => table,
        Err(_) => return out,
    };

    out.base_url = crate::codex_config::extract_codex_base_url(cfg);
    out.model = table
        .get("model")
        .and_then(|value| value.as_str())
        .map(String::from);

    let active_section = table
        .get("model_provider")
        .and_then(|value| value.as_str())
        .and_then(|key| {
            table
                .get("model_providers")
                .and_then(|value| value.as_table())
                .and_then(|providers| providers.get(key))
                .and_then(|value| value.as_table())
        });
    let provider_settings =
        active_section.or_else(|| table.get("model_provider").is_none().then_some(&table));

    if let Some(section) = provider_settings {
        out.wire_api = section
            .get("wire_api")
            .and_then(|value| value.as_str())
            .and_then(|value| match value {
                "chat" => Some(CodexWireApi::Chat),
                "responses" => Some(CodexWireApi::Responses),
                _ => None,
            });
        out.requires_openai_auth = section
            .get("requires_openai_auth")
            .and_then(|value| value.as_bool());
        out.env_key = section
            .get("env_key")
            .and_then(|value| value.as_str())
            .map(String::from);
    }

    out
}

pub(crate) fn update_codex_config_snippet(
    original: &str,
    base_url: &str,
    model: &str,
    wire_api: CodexWireApi,
) -> String {
    crate::codex_config::update_codex_config_snippet(original, base_url, model, wire_api.as_str())
}

pub(crate) fn build_codex_third_party_config_toml(
    provider_name: &str,
    base_url: &str,
    model: &str,
    wire_api: CodexWireApi,
) -> String {
    crate::codex_config::build_codex_third_party_config_toml(
        provider_name,
        base_url,
        model,
        wire_api.as_str(),
    )
}

// Match upstream useCodexConfigState: stored auth wins; bearer is a display fallback.
pub(super) fn codex_api_key(settings: &serde_json::Value) -> String {
    settings
        .get("auth")
        .and_then(|auth| auth.get("OPENAI_API_KEY"))
        .and_then(serde_json::Value::as_str)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let doc = settings
                .get("config")?
                .as_str()?
                .parse::<toml_edit::DocumentMut>()
                .ok()?;
            let token = |item: &toml_edit::Item| {
                item.as_str()
                    .map(str::trim)
                    .filter(|key| !key.is_empty())
                    .map(str::to_string)
            };
            doc.get("model_provider")
                .and_then(|item| item.as_str())
                .map(str::trim)
                .filter(|id| !id.is_empty() && !crate::live::project::codex::is_built_in_id(id))
                .and_then(|id| {
                    doc.get("model_providers")?
                        .get(id)?
                        .get("experimental_bearer_token")
                })
                .and_then(token)
                .or_else(|| doc.get("experimental_bearer_token").and_then(token))
        })
        .unwrap_or_default()
}

// Match upstream updateCodexExperimentalBearerToken: update only an existing
// active-provider token, falling back to the root token. Never add a token.
pub(super) fn update_existing_codex_bearer(config: &str, key: &str) -> String {
    let Ok(mut doc) = config.parse::<toml_edit::DocumentMut>() else {
        return config.to_string();
    };
    let provider = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty() && !crate::live::project::codex::is_built_in_id(id))
        .filter(|id| {
            doc.get("model_providers")
                .and_then(|item| item.get(*id))
                .and_then(|item| item.get("experimental_bearer_token"))
                .is_some()
        })
        .map(str::to_string);
    let table = match provider {
        Some(id) => doc["model_providers"][&id].as_table_like_mut(),
        None => Some(doc.as_table_mut() as &mut dyn toml_edit::TableLike),
    };
    if let Some(table) = table {
        if let Some(token) = table.get_mut("experimental_bearer_token") {
            if key.trim().is_empty() {
                table.remove("experimental_bearer_token");
            } else {
                *token = toml_edit::value(key.trim());
            }
        }
    }
    doc.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_snippet_uses_active_provider_over_stale_root() {
        let parsed = parse_codex_config_snippet(
            r#"base_url = "https://stale.example.com/v1"
model_provider = "current"

[model_providers.current]
base_url = "https://current.example.com/v1"
"#,
        );

        assert_eq!(
            parsed.base_url.as_deref(),
            Some("https://current.example.com/v1")
        );
    }

    #[test]
    fn parse_snippet_supports_legacy_flat_base_url() {
        let parsed = parse_codex_config_snippet(
            r#"base_url = "https://legacy.example.com/v1"
model = "gpt-legacy"
wire_api = "chat"
requires_openai_auth = false
env_key = "LEGACY_API_KEY"
"#,
        );

        assert_eq!(
            parsed.base_url.as_deref(),
            Some("https://legacy.example.com/v1")
        );
        assert_eq!(parsed.model.as_deref(), Some("gpt-legacy"));
        assert_eq!(parsed.wire_api, Some(CodexWireApi::Chat));
        assert_eq!(parsed.requires_openai_auth, Some(false));
        assert_eq!(parsed.env_key.as_deref(), Some("LEGACY_API_KEY"));
    }
}
