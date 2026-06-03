use axum::http::HeaderMap;
use serde_json::Value;

use crate::services::{CodexOAuthService, CopilotAuthService};
use crate::{app_config::AppType, provider::Provider};

use super::super::{
    body_filter::filter_private_params_with_whitelist,
    copilot_optimizer,
    error::ProxyError,
    http_client,
    json_canonical::canonicalize_value,
    model_mapper::{apply_model_mapping, strip_one_m_suffix_for_upstream_from_body},
    providers::{
        apply_codex_chat_upstream_model, claude_api_format_needs_transform, copilot_auth,
        get_adapter, normalize_anthropic_tool_thinking_history_for_provider,
        resolve_codex_chat_reasoning_config, should_convert_codex_responses_to_chat,
        transform_codex_chat, AuthStrategy, ProviderAdapter,
    },
    session,
};
use super::{ForwardOptions, RequestForwarder};

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

const COPILOT_FINGERPRINT_HEADERS: &[&str] = &[
    "user-agent",
    "editor-version",
    "editor-plugin-version",
    "copilot-integration-id",
    "x-github-api-version",
    "openai-intent",
    "x-initiator",
    "x-interaction-type",
    "x-interaction-id",
    "x-vscode-user-agent-library-version",
    "x-request-id",
    "x-agent-task-id",
];

struct CopilotOptimization {
    classification: copilot_optimizer::CopilotClassification,
    deterministic_request_id: Option<String>,
    interaction_id: Option<String>,
    request_classification: bool,
}

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
        let is_full_url = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.is_full_url)
            .unwrap_or(false);
        let is_copilot = is_claude_request
            && (provider.is_github_copilot() || base_url.contains("githubcopilot.com"));
        let (mut mapped_body, _, _) = apply_model_mapping(body.clone(), provider);
        let codex_responses_to_chat = should_convert_codex_responses_to_chat(provider, endpoint)
            && matches!(app_type, AppType::Codex);

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
            mapped_body =
                super::super::providers::copilot_model_map::apply_copilot_model_normalization(
                    mapped_body,
                );
            self.apply_copilot_live_model_resolution(provider, &mut mapped_body)
                .await;
        } else {
            mapped_body = strip_one_m_suffix_for_upstream_from_body(mapped_body);
        }

        let copilot_optimization = if is_copilot && self.copilot_optimizer_config.enabled {
            let classification = copilot_optimizer::classify_request(
                &mapped_body,
                headers.contains_key("anthropic-beta"),
                self.copilot_optimizer_config.compact_detection,
                self.copilot_optimizer_config.subagent_detection,
            );
            log::debug!(
                "[Copilot] optimizer classification: initiator={}, is_warmup={}, is_compact={}, is_subagent={}",
                classification.initiator,
                classification.is_warmup,
                classification.is_compact,
                classification.is_subagent
            );

            mapped_body = copilot_optimizer::sanitize_orphan_tool_results(mapped_body);

            if self.copilot_optimizer_config.tool_result_merging {
                mapped_body = copilot_optimizer::merge_tool_results(mapped_body);
            }

            if self.copilot_optimizer_config.strip_thinking {
                mapped_body = copilot_optimizer::strip_thinking_blocks(mapped_body);
            }

            if self.copilot_optimizer_config.warmup_downgrade && classification.is_warmup {
                mapped_body["model"] =
                    serde_json::json!(&self.copilot_optimizer_config.warmup_model);
            }

            let copilot_session_id = copilot_optimizer_session_id(body, headers);
            let deterministic_request_id = self
                .copilot_optimizer_config
                .deterministic_request_id
                .then(|| {
                    copilot_optimizer::deterministic_request_id(&mapped_body, &copilot_session_id)
                });
            let interaction_id =
                copilot_optimizer::deterministic_interaction_id(&copilot_session_id);

            Some(CopilotOptimization {
                classification,
                deterministic_request_id,
                interaction_id,
                request_classification: self.copilot_optimizer_config.request_classification,
            })
        } else {
            None
        };

        if is_copilot && !is_full_url {
            let dynamic_endpoint = match provider
                .meta
                .as_ref()
                .and_then(|meta| meta.managed_account_id_for("github_copilot"))
            {
                Some(account_id) => CopilotAuthService::get_api_endpoint(&account_id).await,
                None => CopilotAuthService::get_default_api_endpoint().await,
            };
            if dynamic_endpoint != base_url {
                base_url = dynamic_endpoint;
            }
        }

        let claude_api_format = if is_claude_request {
            Some(
                self.resolve_claude_api_format(provider, &mapped_body, is_copilot)
                    .await,
            )
        } else {
            None
        };
        if is_claude_request {
            if let Some(api_format) = claude_api_format.as_deref() {
                normalize_anthropic_tool_thinking_history_for_provider(
                    &mut mapped_body,
                    provider,
                    api_format,
                );
            }
        }
        let needs_transform = match claude_api_format.as_deref() {
            Some(api_format) => claude_api_format_needs_transform(api_format),
            None => adapter.needs_transform(provider),
        };

        if is_claude_request && needs_transform {
            upstream_endpoint = rewrite_claude_transform_endpoint(
                endpoint,
                claude_api_format.as_deref().unwrap_or("anthropic"),
                is_copilot,
                &mapped_body,
            );
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
                super::super::providers::transform_claude_request_for_api_format_with_shadow(
                    mapped_body,
                    provider,
                    claude_api_format.as_deref().unwrap_or("anthropic"),
                    self.session_client_provided
                        .then_some(self.session_id.as_str()),
                    self.gemini_shadow.as_deref(),
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
            is_copilot,
            self.session_client_provided
                .then_some(self.session_id.as_str()),
            force_identity_encoding,
            claude_api_format.as_deref(),
            copilot_optimization.as_ref(),
        )
        .await
    }

    async fn resolve_claude_api_format(
        &self,
        provider: &Provider,
        body: &Value,
        is_copilot: bool,
    ) -> String {
        if !is_copilot {
            return super::super::providers::get_claude_api_format(provider).to_string();
        }

        let model = body.get("model").and_then(|value| value.as_str());
        if let Some(model_id) = model {
            if self
                .is_copilot_openai_vendor_model(provider, model_id)
                .await
            {
                return "openai_responses".to_string();
            }
        }

        "openai_chat".to_string()
    }

    async fn apply_copilot_live_model_resolution(&self, provider: &Provider, body: &mut Value) {
        let Some(model_id) = body.get("model").and_then(|value| value.as_str()) else {
            return;
        };
        let model_id = model_id.to_string();
        let account_id = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.managed_account_id_for("github_copilot"));

        let models_result = match account_id.as_deref() {
            Some(id) => CopilotAuthService::fetch_models_for_account(id).await,
            None => CopilotAuthService::fetch_models().await,
        };
        let Ok(models) = models_result else {
            return;
        };

        if let Some(resolved) =
            super::super::providers::copilot_model_map::resolve_against_models(&model_id, &models)
        {
            body["model"] = Value::String(resolved);
        }
    }

    async fn is_copilot_openai_vendor_model(&self, provider: &Provider, model_id: &str) -> bool {
        let account_id = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.managed_account_id_for("github_copilot"));

        let vendor_result = match account_id.as_deref() {
            Some(id) => CopilotAuthService::get_model_vendor_for_account(id, model_id).await,
            None => CopilotAuthService::get_model_vendor(model_id).await,
        };

        matches!(vendor_result, Ok(Some(vendor)) if vendor.eq_ignore_ascii_case("openai"))
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

fn copilot_optimizer_session_id(body: &Value, headers: &HeaderMap) -> String {
    let metadata = body.get("metadata");
    metadata
        .and_then(|m| m.get("user_id"))
        .and_then(|v| v.as_str())
        .and_then(session::parse_session_from_user_id)
        .or_else(|| {
            metadata
                .and_then(|m| m.get("session_id"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            metadata
                .and_then(|m| m.get("user_id"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            headers
                .get("x-session-id")
                .and_then(|v| v.to_str().ok())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_default()
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
    is_copilot: bool,
    client_session_id: Option<&str>,
    force_identity_encoding: bool,
    claude_api_format: Option<&str>,
    copilot_optimization: Option<&CopilotOptimization>,
) -> Result<reqwest::RequestBuilder, ProxyError> {
    let (endpoint_path, endpoint_query) = split_endpoint_and_query(endpoint);
    let url = if claude_api_format == Some("gemini_native") {
        let is_full_url = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.is_full_url)
            .unwrap_or(false);
        super::super::gemini_url::resolve_gemini_native_url(base_url, endpoint, is_full_url)
    } else if provider
        .meta
        .as_ref()
        .and_then(|meta| meta.is_full_url)
        .unwrap_or(false)
    {
        append_query_to_url(base_url.trim_end_matches('/'), endpoint_query)
    } else if base_url
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
        if key.as_str().eq_ignore_ascii_case("accept-encoding") {
            if !force_identity_encoding {
                request = request.header(key, value);
            }
            continue;
        }

        if HEADER_BLACKLIST
            .iter()
            .any(|blocked| key.as_str().eq_ignore_ascii_case(blocked))
            || (is_copilot && is_copilot_fingerprint_header(key.as_str()))
        {
            continue;
        }
        request = request.header(key, value);
    }

    let send_anthropic_headers = is_claude_request && claude_api_format == Some("anthropic");

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

    if let Some(auth) = adapter.extract_auth(provider) {
        let mut effective_auth = auth.clone();
        if auth.strategy == AuthStrategy::GitHubCopilot {
            let account_id = provider
                .meta
                .as_ref()
                .and_then(|meta| meta.managed_account_id_for("github_copilot"));

            match match &account_id {
                Some(id) => CopilotAuthService::get_valid_token_for_account(id).await,
                None => CopilotAuthService::get_valid_token().await,
            } {
                Ok(token) => {
                    effective_auth.api_key = token;
                    request = add_copilot_auth_headers(
                        request,
                        &effective_auth.api_key,
                        copilot_optimization,
                    );
                }
                Err(error) => {
                    return Err(ProxyError::AuthError(format!(
                        "GitHub Copilot 认证失败: {error}"
                    )));
                }
            }
        } else if auth.strategy == AuthStrategy::CodexOAuth {
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

    reject_proxy_placeholder_for_managed_account_upstream(&request)?;
    Ok(request.json(request_body))
}

fn add_copilot_auth_headers(
    request: reqwest::RequestBuilder,
    api_key: &str,
    optimization: Option<&CopilotOptimization>,
) -> reqwest::RequestBuilder {
    let request_id = optimization
        .and_then(|state| state.deterministic_request_id.as_deref())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let initiator = optimization
        .filter(|state| state.request_classification)
        .map(|state| state.classification.initiator)
        .unwrap_or("user");
    let interaction_type = optimization
        .filter(|state| state.classification.is_subagent)
        .map(|_| "conversation-subagent")
        .unwrap_or("conversation-agent");

    let mut request = request
        .header("Authorization", format!("Bearer {api_key}"))
        .header("editor-version", copilot_auth::COPILOT_EDITOR_VERSION)
        .header(
            "editor-plugin-version",
            copilot_auth::COPILOT_PLUGIN_VERSION,
        )
        .header(
            "copilot-integration-id",
            copilot_auth::COPILOT_INTEGRATION_ID,
        )
        .header("user-agent", copilot_auth::COPILOT_USER_AGENT)
        .header("x-github-api-version", copilot_auth::COPILOT_API_VERSION)
        .header("openai-intent", "conversation-agent")
        .header("x-initiator", initiator)
        .header("x-interaction-type", interaction_type)
        .header("x-vscode-user-agent-library-version", "electron-fetch")
        .header("x-request-id", &request_id)
        .header("x-agent-task-id", request_id);

    if let Some(interaction_id) = optimization.and_then(|state| state.interaction_id.as_deref()) {
        request = request.header("x-interaction-id", interaction_id);
    }

    request
}

fn is_copilot_fingerprint_header(name: &str) -> bool {
    COPILOT_FINGERPRINT_HEADERS
        .iter()
        .any(|header| name.eq_ignore_ascii_case(header))
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

fn strip_beta_query(query: Option<&str>) -> Option<String> {
    let filtered = query.map(|query| {
        query
            .split('&')
            .filter(|pair| !pair.is_empty() && !pair.starts_with("beta="))
            .collect::<Vec<_>>()
            .join("&")
    });

    match filtered.as_deref() {
        Some("") | None => None,
        Some(_) => filtered,
    }
}

fn is_claude_messages_path(path: &str) -> bool {
    matches!(path, "/v1/messages" | "/claude/v1/messages")
}

fn rewrite_claude_transform_endpoint(
    endpoint: &str,
    api_format: &str,
    is_copilot: bool,
    body: &Value,
) -> String {
    let (path, query) = split_endpoint_and_query(endpoint);
    if !is_claude_messages_path(path) {
        return endpoint.to_string();
    }

    let query = strip_beta_query(query);

    if api_format == "gemini_native" {
        let model = super::super::providers::transform_gemini::extract_gemini_model(body)
            .map(super::super::gemini_url::normalize_gemini_model_id)
            .unwrap_or("unknown");
        let is_stream = body
            .get("stream")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let target = if is_stream {
            format!("/v1beta/models/{model}:streamGenerateContent")
        } else {
            format!("/v1beta/models/{model}:generateContent")
        };
        let query = merge_query_params(query.as_deref(), is_stream.then_some("alt=sse"));
        return match query {
            Some(query) if !query.is_empty() => format!("{target}?{query}"),
            _ => target,
        };
    }

    let target = if is_copilot && api_format == "openai_responses" {
        "/v1/responses"
    } else if is_copilot {
        "/chat/completions"
    } else if api_format == "openai_responses" {
        "/v1/responses"
    } else {
        "/v1/chat/completions"
    };

    match query {
        Some(query) if !query.is_empty() => format!("{target}?{query}"),
        _ => target.to_string(),
    }
}

fn merge_query_params(base_query: Option<&str>, extra_param: Option<&str>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(query) = base_query.map(str::trim).filter(|query| !query.is_empty()) {
        parts.push(query.to_string());
    }
    if let Some(param) = extra_param.map(str::trim).filter(|param| !param.is_empty()) {
        let key = param.split_once('=').map_or(param, |(key, _)| key);
        if !parts
            .iter()
            .flat_map(|query| query.split('&'))
            .any(|existing| existing.split_once('=').map_or(existing, |(key, _)| key) == key)
        {
            parts.push(param.to_string());
        }
    }
    (!parts.is_empty()).then(|| parts.join("&"))
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
