use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

use crate::app_config::AppType;

use super::{
    forwarder::{ForwardOptions, RequestForwarder},
    handler_context::HandlerContext,
    metrics::estimate_tokens_from_value,
    providers::{ClaudeAdapter, ProviderAdapter},
    response::{
        build_anthropic_stream_response, build_buffered_json_response,
        build_buffered_passthrough_response, build_json_response, build_passthrough_response,
        is_sse_response,
    },
    response_handler::{proxy_error_response, ResponseHandler, SuccessSyncInfo},
    server::ProxyServerState,
    types::RectifierConfig,
    usage::{log_error_request, RequestLogContext, UsageLogPolicy},
};

pub async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "ok": true })))
}

pub async fn get_status(State(state): State<ProxyServerState>) -> impl IntoResponse {
    Json(state.snapshot_status().await)
}

pub async fn handle_messages(
    State(state): State<ProxyServerState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    handle_claude_request(state, headers, body).await
}

pub async fn handle_chat_completions(
    State(state): State<ProxyServerState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    handle_passthrough_request(
        state,
        headers,
        body,
        AppType::Codex,
        "/chat/completions".to_string(),
    )
    .await
}

pub async fn handle_responses(
    State(state): State<ProxyServerState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    handle_passthrough_request(
        state,
        headers,
        body,
        AppType::Codex,
        "/responses".to_string(),
    )
    .await
}

pub async fn handle_responses_compact(
    State(state): State<ProxyServerState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    handle_passthrough_request(
        state,
        headers,
        body,
        AppType::Codex,
        "/responses/compact".to_string(),
    )
    .await
}

pub async fn handle_gemini(
    State(state): State<ProxyServerState>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let endpoint = uri
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| uri.path().to_string());
    let endpoint = endpoint
        .strip_prefix("/gemini")
        .unwrap_or(endpoint.as_str())
        .to_string();
    handle_passthrough_request(state, headers, body, AppType::Gemini, endpoint).await
}

async fn handle_claude_request(
    state: ProxyServerState,
    headers: HeaderMap,
    body: Value,
) -> Response {
    state
        .record_estimated_input_tokens(estimate_tokens_from_value(&body))
        .await;
    let context = match HandlerContext::load(&state, AppType::Claude, &headers, &body).await {
        Ok(context) => context,
        Err(error) => {
            state.record_request_error(&error).await;
            return proxy_error_response(error);
        }
    };

    let forwarder = match RequestForwarder::new(context.provider_router.clone()) {
        Ok(forwarder) => forwarder.with_optimizer_config(context.optimizer_config.clone()),
        Err(error) => {
            context.state.record_request_error(&error).await;
            return proxy_error_response(error);
        }
    };

    let is_stream = body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let adapter = ClaudeAdapter::new();

    if is_stream {
        let first_byte_timeout = context.streaming_first_byte_timeout();
        let request_started_at = Instant::now();
        let options = ForwardOptions {
            max_retries: context.app_proxy.max_retries,
            request_timeout: first_byte_timeout,
            bypass_circuit_breaker: !context.app_proxy.auto_failover_enabled,
        };
        let forward_result = match forwarder
            .forward_response_detailed(
                &context.app_type,
                "/v1/messages",
                body,
                &headers,
                context.providers().to_vec(),
                options,
                context.rectifier_config.clone(),
            )
            .await
        {
            Ok(response) => response,
            Err(failure) => {
                let super::forwarder::ForwardFailure { provider, error } = failure;
                if let Some(provider) = provider.or_else(|| context.primary_provider().cloned()) {
                    let request_log = RequestLogContext::from_handler(
                        &context,
                        provider,
                        true,
                        UsageLogPolicy::Passthrough,
                    );
                    log_error_request(&context.state, &request_log, &error).await;
                }
                context.state.record_request_error(&error).await;
                return proxy_error_response(error);
            }
        };

        let api_format = super::providers::get_claude_api_format(&forward_result.provider);
        let request_log = RequestLogContext::from_handler(
            &context,
            forward_result.provider.clone(),
            true,
            if adapter.needs_transform(&forward_result.provider) {
                UsageLogPolicy::Transformed
            } else {
                UsageLogPolicy::Passthrough
            },
        );
        let response = forward_result.response;
        let status = response.status();
        let success_sync = status.is_success().then(|| SuccessSyncInfo {
            app_type: context.app_type.clone(),
            provider: forward_result.provider.clone(),
            current_provider_id_at_start: context.current_provider_id_at_start.clone(),
        });
        let first_byte_timeout = remaining_timeout(first_byte_timeout, request_started_at);
        let idle_timeout = context.streaming_idle_timeout();
        let response_result = match response {
            super::forwarder::StreamingResponse::Live(response)
                if adapter.needs_transform(&forward_result.provider) =>
            {
                if is_sse_response(&response) {
                    build_anthropic_stream_response(
                        response,
                        first_byte_timeout,
                        idle_timeout,
                        api_format,
                    )
                } else {
                    build_json_response(response, first_byte_timeout, |body| {
                        adapter.transform_response(body)
                    })
                    .await
                }
            }
            super::forwarder::StreamingResponse::Live(response) => {
                build_passthrough_response(response, first_byte_timeout, idle_timeout).await
            }
            super::forwarder::StreamingResponse::Buffered(response) => {
                if adapter.needs_transform(&forward_result.provider) {
                    build_buffered_json_response(status, &response.headers, response.body, |body| {
                        adapter.transform_response(body)
                    })
                } else {
                    build_buffered_passthrough_response(status, &response.headers, response.body)
                }
            }
        };

        return ResponseHandler::finish_streaming(
            &context.state,
            response_result,
            status,
            success_sync,
            Some(request_log),
        )
        .await;
    }

    let options = ForwardOptions {
        max_retries: context.app_proxy.max_retries,
        request_timeout: context.non_streaming_timeout(),
        bypass_circuit_breaker: !context.app_proxy.auto_failover_enabled,
    };

    let forward_result = match forwarder
        .forward_buffered_response_detailed(
            &context.app_type,
            "/v1/messages",
            body,
            &headers,
            context.providers().to_vec(),
            options,
            context.rectifier_config.clone(),
        )
        .await
    {
        Ok(response) => response,
        Err(failure) => {
            let super::forwarder::ForwardFailure { provider, error } = failure;
            if let Some(provider) = provider.or_else(|| context.primary_provider().cloned()) {
                let request_log = RequestLogContext::from_handler(
                    &context,
                    provider,
                    false,
                    UsageLogPolicy::Passthrough,
                );
                log_error_request(&context.state, &request_log, &error).await;
            }
            context.state.record_request_error(&error).await;
            return proxy_error_response(error);
        }
    };

    let provider = &forward_result.provider;
    let request_log = RequestLogContext::from_handler(
        &context,
        provider.clone(),
        false,
        if adapter.needs_transform(provider) {
            UsageLogPolicy::Transformed
        } else {
            UsageLogPolicy::Passthrough
        },
    );
    let response = forward_result.response;
    let status = response.status;
    let success_sync = status.is_success().then(|| SuccessSyncInfo {
        app_type: context.app_type.clone(),
        provider: provider.clone(),
        current_provider_id_at_start: context.current_provider_id_at_start.clone(),
    });
    let response_result = if adapter.needs_transform(provider) {
        build_buffered_json_response(status, &response.headers, response.body, |body| {
            adapter.transform_response(body)
        })
    } else {
        build_buffered_passthrough_response(status, &response.headers, response.body)
    };

    ResponseHandler::finish_buffered(
        &context.state,
        response_result,
        status,
        success_sync,
        Some(request_log),
    )
    .await
}

async fn handle_passthrough_request(
    state: ProxyServerState,
    headers: HeaderMap,
    body: Value,
    app_type: AppType,
    endpoint: String,
) -> Response {
    state
        .record_estimated_input_tokens(estimate_tokens_from_value(&body))
        .await;
    let context = match HandlerContext::load(&state, app_type, &headers, &body).await {
        Ok(context) => context,
        Err(error) => {
            state.record_request_error(&error).await;
            return proxy_error_response(error);
        }
    };

    let forwarder = match RequestForwarder::new(context.provider_router.clone()) {
        Ok(forwarder) => forwarder.with_optimizer_config(context.optimizer_config.clone()),
        Err(error) => {
            context.state.record_request_error(&error).await;
            return proxy_error_response(error);
        }
    };

    let is_stream = request_is_streaming(&context.app_type, &endpoint, &body);
    let options = if is_stream {
        ForwardOptions {
            max_retries: context.app_proxy.max_retries,
            request_timeout: context.streaming_first_byte_timeout(),
            bypass_circuit_breaker: !context.app_proxy.auto_failover_enabled,
        }
    } else {
        ForwardOptions {
            max_retries: context.app_proxy.max_retries,
            request_timeout: context.non_streaming_timeout(),
            bypass_circuit_breaker: !context.app_proxy.auto_failover_enabled,
        }
    };

    if is_stream {
        let first_byte_timeout = context.streaming_first_byte_timeout();
        let request_started_at = Instant::now();
        let forward_result = match forwarder
            .forward_response(
                &context.app_type,
                &endpoint,
                body,
                &headers,
                context.providers().to_vec(),
                options,
                RectifierConfig::default(),
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                context.state.record_request_error(&error).await;
                return proxy_error_response(error);
            }
        };

        let response = forward_result.response;
        let status = response.status();
        let success_sync = status.is_success().then(|| SuccessSyncInfo {
            app_type: context.app_type.clone(),
            provider: forward_result.provider.clone(),
            current_provider_id_at_start: context.current_provider_id_at_start.clone(),
        });
        let response_result = match response {
            super::forwarder::StreamingResponse::Live(response) => {
                build_passthrough_response(
                    response,
                    remaining_timeout(first_byte_timeout, request_started_at),
                    context.streaming_idle_timeout(),
                )
                .await
            }
            super::forwarder::StreamingResponse::Buffered(response) => {
                build_buffered_passthrough_response(status, &response.headers, response.body)
            }
        };
        return ResponseHandler::finish_streaming(
            &context.state,
            response_result,
            status,
            success_sync,
            None,
        )
        .await;
    }

    let forward_result = match forwarder
        .forward_buffered_response(
            &context.app_type,
            &endpoint,
            body,
            &headers,
            context.providers().to_vec(),
            options,
            RectifierConfig::default(),
        )
        .await
    {
        Ok(response) => response,
        Err(error) => {
            context.state.record_request_error(&error).await;
            return proxy_error_response(error);
        }
    };

    let response = forward_result.response;
    let success_sync = response.status.is_success().then(|| SuccessSyncInfo {
        app_type: context.app_type.clone(),
        provider: forward_result.provider.clone(),
        current_provider_id_at_start: context.current_provider_id_at_start.clone(),
    });
    ResponseHandler::finish_buffered(
        &context.state,
        build_buffered_passthrough_response(response.status, &response.headers, response.body),
        response.status,
        success_sync,
        None,
    )
    .await
}

fn request_is_streaming(app_type: &AppType, endpoint: &str, body: &Value) -> bool {
    if body
        .get("stream")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return true;
    }

    matches!(app_type, AppType::Gemini)
        && (endpoint.contains("alt=sse") || endpoint.contains(":streamGenerateContent"))
}

fn remaining_timeout(timeout: Option<Duration>, started_at: Instant) -> Option<Duration> {
    timeout.map(|timeout| timeout.saturating_sub(started_at.elapsed()))
}
