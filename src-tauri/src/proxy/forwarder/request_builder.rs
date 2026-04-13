use axum::http::HeaderMap;
use serde_json::Value;

use crate::services::CodexOAuthService;
use crate::{app_config::AppType, provider::Provider};

use super::super::{
    body_filter::filter_private_params_with_whitelist,
    error::ProxyError,
    http_client,
    model_mapper::apply_model_mapping,
    providers::{get_adapter, AuthStrategy, ProviderAdapter},
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
        let upstream_endpoint = self.router.upstream_endpoint(app_type, provider, endpoint);
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

        let request_body = if adapter.needs_transform(provider) {
            adapter.transform_request(mapped_body, provider)?
        } else {
            mapped_body
        };
        let filtered_body = filter_private_params_with_whitelist(request_body, &[]);
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
) -> Result<reqwest::RequestBuilder, ProxyError> {
    let mut request = client.post(adapter.build_url(base_url, endpoint));

    for (key, value) in headers {
        if HEADER_BLACKLIST
            .iter()
            .any(|blocked| key.as_str().eq_ignore_ascii_case(blocked))
        {
            continue;
        }
        request = request.header(key, value);
    }

    if is_claude_request {
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

    if is_claude_request {
        let version = headers
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("2023-06-01");
        request = request.header("anthropic-version", version);
    }

    Ok(request.json(request_body))
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
