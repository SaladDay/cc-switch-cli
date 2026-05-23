use axum::http::HeaderMap;
use serde_json::Value;

use crate::services::CopilotService;
use crate::{app_config::AppType, provider::Provider};

use super::super::{
    body_filter::filter_private_params_with_whitelist,
    copilot_optimizer::{
        classify_request, deterministic_interaction_id, deterministic_request_id,
        merge_tool_results, sanitize_orphan_tool_results, strip_thinking_blocks,
        CopilotClassification,
    },
    error::ProxyError,
    http_client,
    json_canonical::canonicalize_value,
    model_mapper::apply_model_mapping,
    providers::{
        apply_codex_chat_upstream_model, copilot_model_map::apply_copilot_model_normalization,
        get_adapter, resolve_codex_chat_reasoning_config, should_convert_codex_responses_to_chat,
        transform_codex_chat, AuthStrategy, ProviderAdapter,
    },
};
use super::{ForwardOptions, RequestForwarder};

use crate::services::CodexOAuthService;

const PROXY_AUTH_PLACEHOLDER: &str = "PROXY_MANAGED";

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
    "x-forwarded-host",
    "x-forwarded-port",
    "x-forwarded-proto",
    "forwarded",
    "cf-connecting-ip",
    "cf-ipcountry",
    "cf-ray",
    "cf-visitor",
    "true-client-ip",
    "fastly-client-ip",
    "x-azure-clientip",
    "x-azure-fdid",
    "x-azure-ref",
    "akamai-origin-hop",
    "x-akamai-config-log-detail",
    "x-request-id",
    "x-correlation-id",
    "x-trace-id",
    "x-amzn-trace-id",
    "x-b3-traceid",
    "x-b3-spanid",
    "x-b3-parentspanid",
    "x-b3-sampled",
    "traceparent",
    "tracestate",
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
        let mut base_url = adapter.extract_base_url(provider)?;
        let (mut mapped_body, _, _) = apply_model_mapping(body.clone(), provider);
        let codex_responses_to_chat = should_convert_codex_responses_to_chat(provider, endpoint)
            && matches!(app_type, AppType::Codex);
        let needs_transform = adapter.needs_transform(provider);

        let is_copilot = is_claude_request && is_github_copilot_provider(provider);

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

        if is_copilot {
            mapped_body = apply_copilot_model_normalization(mapped_body);
        }

        let copilot_optimization = if is_copilot && self.copilot_optimizer_config.enabled {
            let has_anthropic_beta = headers.contains_key("anthropic-beta");
            let classification = classify_request(
                &mapped_body,
                has_anthropic_beta,
                self.copilot_optimizer_config.compact_detection,
                self.copilot_optimizer_config.subagent_detection,
            );

            mapped_body = sanitize_orphan_tool_results(mapped_body);
            if self.copilot_optimizer_config.tool_result_merging {
                mapped_body = merge_tool_results(mapped_body);
            }
            if self.copilot_optimizer_config.strip_thinking {
                mapped_body = strip_thinking_blocks(mapped_body);
            }
            if self.copilot_optimizer_config.warmup_downgrade && classification.is_warmup {
                mapped_body["model"] =
                    serde_json::json!(&self.copilot_optimizer_config.warmup_model);
            }

            let session_id = extract_copilot_session_id(body, headers);
            let det_request_id = if self.copilot_optimizer_config.deterministic_request_id {
                Some(deterministic_request_id(&mapped_body, &session_id))
            } else {
                None
            };
            let interaction_id = deterministic_interaction_id(&session_id);

            Some((classification, det_request_id, interaction_id))
        } else {
            None
        };

        if is_copilot {
            let account_id = provider
                .meta
                .as_ref()
                .and_then(|m| m.managed_account_id_for("github_copilot"));
            let dynamic_endpoint = match account_id.as_deref() {
                Some(id) => CopilotService::get_api_endpoint(id).await,
                None => CopilotService::get_default_api_endpoint().await,
            };
            if dynamic_endpoint != base_url {
                base_url = dynamic_endpoint;
            }
        }

        let request_body = if codex_responses_to_chat {
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
        } else if needs_transform {
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
        let force_identity_encoding = needs_transform
            || codex_responses_to_chat
            || is_streaming_request(&upstream_endpoint, &filtered_body, headers);
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
            force_identity_encoding,
            copilot_optimization,
            &self.copilot_optimizer_config,
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

fn is_github_copilot_provider(provider: &Provider) -> bool {
    if let Some(meta) = provider.meta.as_ref() {
        if meta.provider_type.as_deref() == Some("github_copilot") {
            return true;
        }
    }
    provider
        .settings_config
        .get("env")
        .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
        .and_then(|v| v.as_str())
        .map(|url| url.contains("githubcopilot.com"))
        .unwrap_or(false)
}

fn extract_copilot_session_id(body: &Value, headers: &HeaderMap) -> String {
    let metadata = body.get("metadata");
    if let Some(user_id) = metadata
        .and_then(|m| m.get("user_id"))
        .and_then(|v| v.as_str())
    {
        if let Some((_, session_id)) = user_id.split_once("_session_") {
            if !session_id.is_empty() {
                return session_id.to_string();
            }
        }
    }
    if let Some(session_id) = metadata
        .and_then(|m| m.get("session_id"))
        .and_then(|v| v.as_str())
    {
        if !session_id.is_empty() {
            return session_id.to_string();
        }
    }
    if let Some(user_id) = metadata
        .and_then(|m| m.get("user_id"))
        .and_then(|v| v.as_str())
    {
        if !user_id.is_empty() {
            return user_id.to_string();
        }
    }
    if let Some(session_id) = headers.get("x-session-id").and_then(|v| v.to_str().ok()) {
        if !session_id.is_empty() {
            return session_id.to_string();
        }
    }
    String::new()
}

// PLACEHOLDER_BUILD_REQUEST

#[allow(clippy::too_many_arguments)]
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
    force_identity_encoding: bool,
    copilot_optimization: Option<(CopilotClassification, Option<String>, Option<String>)>,
    copilot_optimizer_config: &super::super::types::CopilotOptimizerConfig,
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
    let mut request = client.post(&url);

    for (key, value) in headers {
        if key.as_str().eq_ignore_ascii_case("accept-encoding") {
            if !force_identity_encoding {
                request = request.header(key, value);
            }
            continue;
        }

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

    if force_identity_encoding {
        request = request.header("accept-encoding", "identity");
    }

    // PLACEHOLDER_AUTH_SECTION

    if let Some(auth) = adapter.extract_auth(provider) {
        let mut effective_auth = auth.clone();
        match auth.strategy {
            AuthStrategy::CodexOAuth => {
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
            }
            AuthStrategy::GitHubCopilot => {
                let account_id = provider
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.managed_account_id_for("github_copilot"));

                match match account_id.as_deref() {
                    Some(id) => CopilotService::get_valid_token_for_account(id).await,
                    None => CopilotService::get_valid_token().await,
                } {
                    Ok(token) => {
                        effective_auth.api_key = token;
                        request = adapter.add_auth_headers(request, &effective_auth);
                    }
                    Err(error) => {
                        return Err(ProxyError::AuthError(format!(
                            "GitHub Copilot 认证失败: {error}"
                        )));
                    }
                }
            }
            _ => {
                request = adapter.add_auth_headers(request, &effective_auth);
            }
        }
    }

    if send_anthropic_headers {
        let version = headers
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("2023-06-01");
        request = request.header("anthropic-version", version);
    }

    if let Some((classification, det_request_id, interaction_id)) = copilot_optimization {
        if copilot_optimizer_config.request_classification {
            request = request.header("x-initiator", classification.initiator);
        }
        if classification.is_subagent {
            request = request.header("x-interaction-type", "conversation-subagent");
        }
        if let Some(ref det_id) = det_request_id {
            request = request.header("x-request-id", det_id.as_str());
        }
        if let Some(ref iid) = interaction_id {
            request = request.header("x-interaction-id", iid.as_str());
        }
    }

    reject_proxy_placeholder_for_managed_account_upstream(&request)?;
    Ok(request.json(request_body))
}

// PLACEHOLDER_REMAINING_FNS

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

fn reject_proxy_placeholder_for_managed_account_upstream(
    request: &reqwest::RequestBuilder,
) -> Result<(), ProxyError> {
    let Some(cloned_request) = request.try_clone() else {
        return Ok(());
    };
    let built_request = cloned_request.build().map_err(|error| {
        ProxyError::RequestFailed(format!("build upstream request failed: {error}"))
    })?;

    if !is_managed_account_upstream_url(built_request.url())
        || !headers_contain_proxy_placeholder(built_request.headers())
    {
        return Ok(());
    }

    Err(ProxyError::AuthError(
        "Managed account proxy auth was not resolved; PROXY_MANAGED must not be sent upstream"
            .to_string(),
    ))
}

fn is_managed_account_upstream_url(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };

    host == "githubcopilot.com"
        || host.ends_with(".githubcopilot.com")
        || (host == "chatgpt.com" && url.path().starts_with("/backend-api/codex"))
}

fn headers_contain_proxy_placeholder(headers: &reqwest::header::HeaderMap) -> bool {
    headers.values().any(|value| {
        value
            .to_str()
            .map(|value| value.contains(PROXY_AUTH_PLACEHOLDER))
            .unwrap_or(false)
    })
}

fn is_streaming_request(endpoint: &str, body: &Value, headers: &HeaderMap) -> bool {
    if body
        .get("stream")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return true;
    }

    if endpoint.contains("streamGenerateContent") || endpoint.contains("alt=sse") {
        return true;
    }

    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|accept| accept.contains("text/event-stream"))
        .unwrap_or(false)
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
    use super::*;
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
