/// Project the active Codex provider onto the local proxy.
///
/// Codex itself always speaks Responses to cc-switch. The stored provider may
/// use Anthropic or Chat upstream, but that protocol conversion happens inside
/// the proxy and must never leak into live `config.toml`.
pub(super) fn update_toml_base_url(toml_str: &str, new_url: &str) -> String {
    use toml_edit::DocumentMut;

    let mut doc = match toml_str.parse::<DocumentMut>() {
        Ok(doc) => doc,
        Err(_) => return toml_str.to_string(),
    };

    let model_provider = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::to_string);

    if let Some(provider_key) = model_provider {
        if doc.get("model_providers").is_none() {
            doc["model_providers"] = toml_edit::table();
        }

        if let Some(model_providers) = doc["model_providers"].as_table_like_mut() {
            if model_providers.get(&provider_key).is_none() {
                model_providers.insert(&provider_key, toml_edit::table());
            }

            if let Some(provider_table) = model_providers
                .get_mut(&provider_key)
                .and_then(|item| item.as_table_like_mut())
            {
                provider_table.insert("base_url", toml_edit::value(new_url));
                provider_table.insert("wire_api", toml_edit::value("responses"));
                return doc.to_string();
            }
        }
    }

    doc["base_url"] = toml_edit::value(new_url);
    doc["wire_api"] = toml_edit::value("responses");
    doc.to_string()
}

pub(super) fn remove_loopback_base_url_from_toml(toml_str: &str) -> String {
    use toml_edit::DocumentMut;

    let mut doc = match toml_str.parse::<DocumentMut>() {
        Ok(doc) => doc,
        Err(_) => return toml_str.to_string(),
    };

    let model_provider = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::to_string);

    if let Some(provider_key) = model_provider {
        if let Some(base_url) = doc
            .get("model_providers")
            .and_then(|item| item.as_table_like())
            .and_then(|table| table.get(&provider_key))
            .and_then(|item| item.as_table_like())
            .and_then(|table| table.get("base_url"))
            .and_then(|item| item.as_str())
        {
            if contains_loopback_proxy_url(base_url) {
                if let Some(section) = doc
                    .get_mut("model_providers")
                    .and_then(|item| item.as_table_like_mut())
                    .and_then(|table| table.get_mut(&provider_key))
                    .and_then(|item| item.as_table_like_mut())
                {
                    section.remove("base_url");
                }
            }
        }
    }

    if doc
        .get("base_url")
        .and_then(|item| item.as_str())
        .is_some_and(contains_loopback_proxy_url)
    {
        doc.as_table_mut().remove("base_url");
    }

    doc.to_string()
}

pub(super) fn is_loopback_proxy_url(url: &str) -> bool {
    url.contains("127.0.0.1") || url.contains("localhost") || url.contains("[::1]")
}

pub(super) fn contains_loopback_proxy_url(text: &str) -> bool {
    text.contains("127.0.0.1") || text.contains("localhost") || text.contains("[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_projection_forces_active_provider_to_responses() {
        let input = r#"model_provider = "active"

[model_providers.active]
base_url = "https://anthropic.example/v1"
wire_api = "anthropic"

[model_providers.other]
base_url = "https://other.example/v1"
wire_api = "chat"
"#;

        let updated = update_toml_base_url(input, "http://127.0.0.1:15721/v1");
        let parsed: toml::Value = toml::from_str(&updated).expect("parse projected TOML");
        assert_eq!(
            parsed["model_providers"]["active"]["base_url"].as_str(),
            Some("http://127.0.0.1:15721/v1")
        );
        assert_eq!(
            parsed["model_providers"]["active"]["wire_api"].as_str(),
            Some("responses")
        );
        assert_eq!(
            parsed["model_providers"]["other"]["wire_api"].as_str(),
            Some("chat"),
            "inactive providers must remain untouched"
        );
    }

    #[test]
    fn proxy_projection_supports_inline_and_legacy_flat_configs() {
        let inline = r#"model_provider = "active"
model_providers.active = { base_url = "https://anthropic.example/v1", wire_api = "anthropic" }
"#;
        let updated = update_toml_base_url(inline, "http://localhost:15721/v1");
        let parsed: toml::Value = toml::from_str(&updated).expect("parse inline TOML");
        assert_eq!(
            parsed["model_providers"]["active"]["wire_api"].as_str(),
            Some("responses")
        );

        let flat = r#"base_url = "https://anthropic.example/v1"
wire_api = "anthropic"
"#;
        let updated = update_toml_base_url(flat, "http://[::1]:15721/v1");
        let parsed: toml::Value = toml::from_str(&updated).expect("parse flat TOML");
        assert_eq!(parsed["wire_api"].as_str(), Some("responses"));
        assert_eq!(parsed["base_url"].as_str(), Some("http://[::1]:15721/v1"));
    }
}
