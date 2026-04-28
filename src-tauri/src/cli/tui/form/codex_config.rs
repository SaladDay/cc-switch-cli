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

    out.model = table
        .get("model")
        .and_then(|value| value.as_str())
        .map(String::from);

    let section = table
        .get("model_provider")
        .and_then(|value| value.as_str())
        .and_then(|key| {
            table
                .get("model_providers")
                .and_then(|value| value.as_table())
                .and_then(|providers| providers.get(key))
                .and_then(|value| value.as_table())
        });

    if let Some(section) = section {
        out.base_url = section
            .get("base_url")
            .and_then(|value| value.as_str())
            .map(String::from);
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
    requires_openai_auth: bool,
    env_key: &str,
) -> String {
    let mut doc = match original.trim().parse::<toml_edit::DocumentMut>() {
        Ok(doc) => doc,
        Err(_) => return original.to_string(),
    };

    if let Some(model) = non_empty(model) {
        doc["model"] = toml_edit::value(model);
    } else {
        doc.remove("model");
    }

    let provider_key = doc
        .get("model_provider")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());

    if let Some(key) = provider_key {
        if doc.get("model_providers").is_none() {
            doc["model_providers"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let providers = doc["model_providers"]
            .as_table_like_mut()
            .expect("model_providers should be a table");
        if providers.get(&key).is_none() {
            providers.insert(&key, toml_edit::Item::Table(toml_edit::Table::new()));
        }

        if let Some(section) = providers
            .get_mut(&key)
            .and_then(|value| value.as_table_like_mut())
        {
            if let Some(base_url) = non_empty(base_url) {
                section.insert("base_url", toml_edit::value(base_url));
            } else {
                section.remove("base_url");
            }

            section.insert("wire_api", toml_edit::value(wire_api.as_str()));
            section.insert(
                "requires_openai_auth",
                toml_edit::value(requires_openai_auth),
            );

            if requires_openai_auth {
                section.remove("env_key");
            } else {
                let env_key = non_empty(env_key).unwrap_or("OPENAI_API_KEY");
                section.insert("env_key", toml_edit::value(env_key));
            }
        }
    }

    let result = doc.to_string();
    let trimmed = result.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn clean_codex_provider_key(provider_id: &str, provider_name: &str) -> String {
    let raw = if provider_id.trim().is_empty() {
        provider_name.trim()
    } else {
        provider_id.trim()
    };
    crate::codex_config::clean_codex_provider_key(raw)
}

pub(crate) fn build_codex_provider_config_toml(
    provider_key: &str,
    base_url: &str,
    model: &str,
    wire_api: CodexWireApi,
) -> String {
    let provider_key = escape_toml_string(provider_key);
    let model = escape_toml_string(model);
    let base_url = escape_toml_string(base_url);

    [
        format!("model_provider = \"{}\"", provider_key),
        format!("model = \"{}\"", model),
        "model_reasoning_effort = \"high\"".to_string(),
        "disable_response_storage = true".to_string(),
        String::new(),
        format!("[model_providers.{}]", provider_key),
        format!("name = \"{}\"", provider_key),
        format!("base_url = \"{}\"", base_url),
        format!("wire_api = \"{}\"", wire_api.as_str()),
        "requires_openai_auth = true".to_string(),
        String::new(),
    ]
    .join("\n")
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn escape_toml_string(value: &str) -> String {
    value.replace('"', "\\\"")
}
