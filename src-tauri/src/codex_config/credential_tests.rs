use super::{
    extract_codex_api_key, prepare_codex_provider_live_config,
    remove_codex_experimental_bearer_token_if, restore_codex_provider_token_for_backfill,
    sanitize_codex_third_party_auth,
};
use serde_json::json;
use toml_edit::DocumentMut;

#[test]
fn credential_sources_keep_cli_grammar_and_live_before_stored_precedence() {
    let fallback = json!({"OPENAI_API_KEY":"stored", "tokens":{"access_token":"not-a-key"}});
    for (auth, config, expected) in [
        (json!({"OPENAI_API_KEY":" live ","tokens":{"access_token":"oauth"}}), "invalid = [", Some("live")),
        (json!({"OPENAI_API_KEY":false}), "experimental_bearer_token = 'config'", Some("config")),
        (json!({"tokens":{"access_token":"oauth"}}), "experimental_bearer_token = \"\\e\"", None),
        (json!(["opaque"]), "model_provider = 'vendor'\nmodel_providers = {vendor = {experimental_bearer_token = 'inline'}}", None),
        (json!(null), "model_provider = 'vendor'\nexperimental_bearer_token = 'root'\n[model_providers.vendor]\nexperimental_bearer_token = ' '", None),
        (json!({"OPENAI_API_KEY":""}), "experimental_bearer_token = 'root'\n[model_providers]\nvendor = {experimental_bearer_token = 'inline'}", Some("root")),
    ] {
        assert_eq!(extract_codex_api_key(Some(&auth), Some(config)).as_deref(), expected);
        assert_eq!(sanitize_codex_third_party_auth(Some(&auth), Some(config), Some(&fallback), Some("experimental_bearer_token = 'stored-config'")), json!({"OPENAI_API_KEY":expected.unwrap_or("stored")}));
        if expected.is_none() {
            assert_eq!(prepare_codex_provider_live_config(&auth, config).unwrap(), config);
        }
    }
    assert_eq!(
        sanitize_codex_third_party_auth(
            None,
            None,
            None,
            Some("experimental_bearer_token = 'fallback'")
        ),
        json!({"OPENAI_API_KEY":"fallback"})
    );
    assert_eq!(
        sanitize_codex_third_party_auth(
            None,
            None,
            None,
            Some("experimental_bearer_token = \"\\e\"")
        ),
        json!({})
    );
}

// Test-only oracle for the CLI's previous DocumentMut cleanup. Keeping the
// old parser here checks both the field edits and cross-version serialization.
fn previous_cleanup(source: &str) -> Result<String, String> {
    if source.trim().is_empty() || !source.contains("experimental_bearer_token") {
        return Ok(source.to_owned());
    }
    let mut doc = source
        .parse::<DocumentMut>()
        .map_err(|error| format!("Invalid Codex config.toml: {error}"))?;
    let active = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    if let Some(active) = active {
        if let Some(provider) = doc
            .get_mut("model_providers")
            .and_then(|item| item.as_table_mut())
            .and_then(|providers| providers.get_mut(&active))
            .and_then(|item| item.as_table_mut())
        {
            if provider
                .get("experimental_bearer_token")
                .and_then(|item| item.as_str())
                .is_some_and(|token| token.trim() == "remove-me")
            {
                provider.remove("experimental_bearer_token");
            }
        }
    }
    if doc
        .get("experimental_bearer_token")
        .and_then(|item| item.as_str())
        .is_some_and(|token| token.trim() == "remove-me")
    {
        doc.as_table_mut().remove("experimental_bearer_token");
    }
    Ok(doc.to_string())
}

#[test]
fn credential_cleanup_matches_cli_document_semantics_and_errors() {
    for source in [
        "", "  ", "invalid = [", "experimental_bearer_token = [",
        "experimental_bearer_token = \"\\e\"",
        "# heading\nexperimental_bearer_token = ' remove-me '\nfuture = {keep = true} # keep\n",
        "experimental_bearer_token = false\n",
        "model_provider = ' vendor '\nexperimental_bearer_token = 'keep'\n[model_providers.vendor]\nexperimental_bearer_token = 'remove-me'\n[model_providers.inactive]\nexperimental_bearer_token = 'remove-me'\n",
        "model_provider = ' OPENAI '\nexperimental_bearer_token = 'remove-me'\n[model_providers.OPENAI]\nexperimental_bearer_token = 'remove-me'\n",
        "model_provider = 'vendor'\nmodel_providers = {vendor = {experimental_bearer_token = 'remove-me'}}\n",
        "model_provider = 'vendor'\nexperimental_bearer_token = 'remove-me'\n[model_providers]\nvendor = {experimental_bearer_token = 'remove-me'}\n",
        "model_provider = '供应商'\n[model_providers.'供应商']\nexperimental_bearer_token = 'remove-me'\nfuture = [1, 2]\n",
    ] {
        let actual = remove_codex_experimental_bearer_token_if(source, |token| token == "remove-me");
        match previous_cleanup(source) {
            Ok(expected) => assert_eq!(actual.unwrap(), expected, "{source}"),
            Err(expected) => assert_eq!(actual.unwrap_err().to_string(), expected, "{source}"),
        }
    }
}

#[test]
fn provider_auth_roundtrip_keeps_template_extensions_and_native_side_fields() {
    let template = json!({"auth":{"future":{"keep":true}}, "modelCatalog":{"models":[]}});
    let original = "# keep\nmodel_provider = 'vendor'\n[model_providers.vendor]\nbase_url = 'https://example.test'\n[model_providers.inactive]\nexperimental_bearer_token = 'inactive'\n";
    for token in [
        "plain",
        "quote\"slash\\",
        "multi\nline",
        "中文🔑",
        "prefix\u{1b}suffix",
    ] {
        let config =
            prepare_codex_provider_live_config(&json!({"OPENAI_API_KEY":token}), original).unwrap();
        let mut live = json!({"auth":{"tokens":{"access_token":"live-oauth"}}, "config":config, "modelCatalog":{"models":[]}, "extension":42});
        restore_codex_provider_token_for_backfill(&mut live, &template).unwrap();
        assert_eq!(
            live["auth"],
            json!({"OPENAI_API_KEY":token,"future":{"keep":true}})
        );
        assert_eq!(live["config"], original);
        assert_eq!(live["extension"], 42);
        assert_eq!(live["modelCatalog"], template["modelCatalog"]);
    }
    for mut live in [
        json!(null),
        json!({"config":false}),
        json!({"auth":{"keep":true},"config":"experimental_bearer_token = \"\\e\""}),
    ] {
        let before = live.clone();
        restore_codex_provider_token_for_backfill(&mut live, &template).unwrap();
        assert_eq!(live, before);
    }
}
