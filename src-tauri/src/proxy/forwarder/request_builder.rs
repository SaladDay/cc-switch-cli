use axum::http::HeaderMap;
use serde_json::Value;

use crate::services::CodexOAuthService;
use crate::{app_config::AppType, provider::Provider};

use super::super::{
    body_filter::filter_private_params_with_whitelist,
    error::ProxyError,
    http_client,
    json_canonical::canonicalize_value,
    model_mapper::apply_model_mapping,
    providers::{
        apply_codex_chat_upstream_model, get_adapter, resolve_codex_chat_reasoning_config,
        should_convert_codex_responses_to_chat, transform_codex_chat, AuthStrategy,
        ProviderAdapter,
    },
};
use super::{ForwardOptions, RequestForwarder};

const HEADER_BLACKLIST: &[&str] = &[
    "authorization",
    "x-api-key",
    "x-goog-api-key",
    "host",
    "content-length",
    "transfer-encoding",
    "accept-encoding",
    "anthropic-beta",
    "anthropic-version",
    "x-forwarded-for",
    "x-real-ip",
];

impl RequestForwarder {
    pub(super) async fn prepare_request(
        &self,
        app_type: &AppType,
        provider: &Provider,
        endpoint: &str,
        body: &Value,
        headers: &HeaderMap,
        options: ForwardOptions,
    ) -> Result<reqwest::RequestBuilder, ProxyError> {
        let adapter = get_adapter(app_type);
        let is_claude_request = matches!(app_type, AppType::Claude);
        let mut upstream_endpoint = self.router.upstream_endpoint(app_type, provider, endpoint);
        let base_url = adapter.extract_base_url(provider)?;
        let (mut mapped_body, _, _) = apply_model_mapping(body.clone(), provider);

        if is_claude_request && self.optimizer_config.enabled && is_bedrock_provider(provider) {
            if self.optimizer_config.thinking_optimizer {
                super::super::thinking_optimizer::optimize(
                    &mut mapped_body,
                    &self.optimizer_config,
                );
            }
            if self.optimizer_config.cache_injection {
                super::super::cache_injector::inject(&mut mapped_body, &self.optimizer_config);
            }
        }

        let request_body = if should_convert_codex_responses_to_chat(provider, endpoint)
            && matches!(app_type, AppType::Codex)
        {
            upstream_endpoint = rewrite_codex_responses_endpoint_to_chat(endpoint);
            if let Some(history) = self.codex_chat_history.as_ref() {
                history.enrich_request(&mut mapped_body).await;
            }
            apply_codex_chat_upstream_model(provider, &mut mapped_body);
            let reasoning_config = resolve_codex_chat_reasoning_config(provider, &mapped_body);
            transform_codex_chat::responses_to_chat_completions_with_reasoning(
                mapped_body,
                reasoning_config.as_ref(),
            )?
        } else if adapter.needs_transform(provider) {
            if is_claude_request {
                super::super::providers::transform_claude_request_for_api_format(
                    mapped_body,
                    provider,
                    super::super::providers::get_claude_api_format(provider),
                    self.session_client_provided
                        .then_some(self.session_id.as_str()),
                )?
            } else {
                adapter.transform_request(mapped_body, provider)?
            }
        } else {
            mapped_body
        };
        let filtered_body = prepare_upstream_request_body(request_body);
        let client = self.client_for_provider(provider);

        build_request(
            &client,
            &*adapter,
            provider,
            &base_url,
            &upstream_endpoint,
            &filtered_body,
            headers,
            options,
            is_claude_request,
            self.session_client_provided
                .then_some(self.session_id.as_str()),
        )
        .await
    }

    fn client_for_provider(&self, provider: &Provider) -> reqwest::Client {
        http_client::get_for_provider(
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.proxy_config.as_ref()),
        )
    }
}

fn prepare_upstream_request_body(request_body: Value) -> Value {
    canonicalize_value(filter_private_params_with_whitelist(request_body, &[]))
}

async fn build_request(
    client: &reqwest::Client,
    adapter: &dyn ProviderAdapter,
    provider: &Provider,
    base_url: &str,
    endpoint: &str,
    request_body: &Value,
    headers: &HeaderMap,
    _options: ForwardOptions,
    is_claude_request: bool,
    client_session_id: Option<&str>,
) -> Result<reqwest::RequestBuilder, ProxyError> {
    let (endpoint_path, endpoint_query) = split_endpoint_and_query(endpoint);
    let url = if base_url
        .trim_end_matches('/')
        .to_ascii_lowercase()
        .ends_with("/chat/completions")
        && endpoint_path.trim_matches('/') == "chat/completions"
    {
        append_query_to_url(base_url.trim_end_matches('/'), endpoint_query)
    } else {
        adapter.build_url(base_url, endpoint)
    };
    let mut request = client.post(url);

    for (key, value) in headers {
        if HEADER_BLACKLIST
            .iter()
            .any(|blocked| key.as_str().eq_ignore_ascii_case(blocked))
        {
            continue;
        }
        request = request.header(key, value);
    }

    let send_anthropic_headers = is_claude_request
        && super::super::providers::get_claude_api_format(provider) == "anthropic";

    if send_anthropic_headers {
        const CLAUDE_CODE_BETA: &str = "claude-code-20250219";
        let beta_value = headers
            .get("anthropic-beta")
            .and_then(|value| value.to_str().ok())
            .map(|value| {
                if value.contains(CLAUDE_CODE_BETA) {
                    value.to_string()
                } else {
                    format!("{CLAUDE_CODE_BETA},{value}")
                }
            })
            .unwrap_or_else(|| CLAUDE_CODE_BETA.to_string());
        request = request.header("anthropic-beta", beta_value);
    }

    if let Some(forwarded_for) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        request = request.header("x-forwarded-for", forwarded_for);
    }
    if let Some(real_ip) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        request = request.header("x-real-ip", real_ip);
    }

    request = request.header("accept-encoding", "identity");

    if let Some(auth) = adapter.extract_auth(provider) {
        let mut effective_auth = auth.clone();
        if auth.strategy == AuthStrategy::CodexOAuth {
            let account_id = provider
                .meta
                .as_ref()
                .and_then(|meta| meta.managed_account_id_for("codex_oauth"));

            match match &account_id {
                Some(id) => CodexOAuthService::get_valid_token_for_account(id).await,
                None => CodexOAuthService::get_valid_token().await,
            } {
                Ok(token) => {
                    effective_auth.api_key = token;
                    request = adapter.add_auth_headers(request, &effective_auth);
                    let resolved_account_id = match account_id {
                        Some(id) => Some(id),
                        None => CodexOAuthService::default_account_id().await,
                    };
                    if let Some(account_id) = resolved_account_id {
                        request = request.header("ChatGPT-Account-Id", account_id);
                    }
                    if let Some(session_id) = client_session_id {
                        for (name, value) in build_codex_oauth_session_headers(session_id) {
                            request = request.header(name, value);
                        }
                    }
                }
                Err(error) => {
                    return Err(ProxyError::AuthError(format!(
                        "Codex OAuth 认证失败: {error}"
                    )));
                }
            }
        } else {
            request = adapter.add_auth_headers(request, &effective_auth);
        }
    }

    if send_anthropic_headers {
        let version = headers
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("2023-06-01");
        request = request.header("anthropic-version", version);
    }

    Ok(request.json(request_body))
}

fn split_endpoint_and_query(endpoint: &str) -> (&str, Option<&str>) {
    endpoint
        .split_once('?')
        .map_or((endpoint, None), |(path, query)| (path, Some(query)))
}

fn rewrite_codex_responses_endpoint_to_chat(endpoint: &str) -> String {
    match split_endpoint_and_query(endpoint).1 {
        Some(query) if !query.is_empty() => format!("/chat/completions?{query}"),
        _ => "/chat/completions".to_string(),
    }
}

fn append_query_to_url(url: &str, query: Option<&str>) -> String {
    let Some(query) = query.filter(|query| !query.is_empty()) else {
        return url.to_string();
    };

    if url.ends_with('?') || url.ends_with('&') {
        format!("{url}{query}")
    } else if url.contains('?') {
        format!("{url}&{query}")
    } else {
        format!("{url}?{query}")
    }
}

fn is_bedrock_provider(provider: &Provider) -> bool {
    provider
        .settings_config
        .get("env")
        .and_then(|env| env.get("CLAUDE_CODE_USE_BEDROCK"))
        .and_then(|value| value.as_str())
        .map(|value| value == "1")
        .unwrap_or(false)
}

fn build_codex_oauth_session_headers(
    session_id: &str,
) -> Vec<(reqwest::header::HeaderName, reqwest::header::HeaderValue)> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Vec::new();
    }

    let mut headers = Vec::new();
    if let Ok(value) = reqwest::header::HeaderValue::from_str(session_id) {
        headers.push((
            reqwest::header::HeaderName::from_static("session_id"),
            value.clone(),
        ));
        headers.push((
            reqwest::header::HeaderName::from_static("x-client-request-id"),
            value,
        ));
    }

    let window_id = format!("{session_id}:0");
    if let Ok(value) = reqwest::header::HeaderValue::from_str(&window_id) {
        headers.push((
            reqwest::header::HeaderName::from_static("x-codex-window-id"),
            value,
        ));
    }

    headers
}

#[cfg(test)]
mod tests {
    use super::prepare_upstream_request_body;
    use serde_json::json;

    #[test]
    fn prepare_upstream_request_body_filters_private_fields_and_canonicalizes_order() {
        let body = json!({
            "z": 1,
            "_internal": "drop",
            "tools": [
                {
                    "name": "lookup",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "_id": {
                                "_private_note": "drop",
                                "type": "string"
                            },
                            "b": {"type": "number"},
                            "a": {"type": "string"}
                        }
                    }
                }
            ],
            "a": 2
        });

        let prepared = prepare_upstream_request_body(body);

        assert!(prepared.get("_internal").is_none());
        assert!(prepared["tools"][0]["parameters"]["properties"]
            .get("_id")
            .is_some());
        assert!(prepared["tools"][0]["parameters"]["properties"]["_id"]
            .get("_private_note")
            .is_none());
        assert_eq!(
            serde_json::to_string(&prepared).expect("serialize prepared body"),
            r#"{"a":2,"tools":[{"name":"lookup","parameters":{"properties":{"_id":{"type":"string"},"a":{"type":"string"},"b":{"type":"number"}},"type":"object"}}],"z":1}"#
        );
    }
}
