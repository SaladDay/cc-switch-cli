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
    crate::codex_config::update_codex_config_snippet(
        original,
        base_url,
        model,
        wire_api.as_str(),
        requires_openai_auth,
        env_key,
    )
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
