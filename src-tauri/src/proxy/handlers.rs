use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use bytes::Bytes;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

use crate::app_config::AppType;

use super::{
    error::ProxyError,
    forwarder::{ForwardOptions, RequestForwarder},
    handler_context::HandlerContext,
    metrics::estimate_tokens_from_value,
    providers::{ClaudeAdapter, ProviderAdapter},
    response::{
        build_anthropic_stream_response, build_buffered_codex_chat_response,
        build_buffered_json_response, build_buffered_passthrough_response,
        build_codex_chat_error_response, build_codex_chat_stream_response, build_json_response,
        build_passthrough_response, is_sse_response, PreparedResponse,
    },
    response_handler::{proxy_error_response, ResponseHandler, SuccessSyncInfo},
    server::ProxyServerState,
    sse::{strip_sse_field, take_sse_block},
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
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    handle_passthrough_request(
        state,
        headers,
        body,
        AppType::Codex,
        endpoint_with_query(&uri, "/chat/completions"),
    )
    .await
}

pub async fn handle_responses(
    State(state): State<ProxyServerState>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    handle_passthrough_request(
        state,
        headers,
        body,
        AppType::Codex,
        endpoint_with_query(&uri, "/responses"),
    )
    .await
}

pub async fn handle_responses_compact(
    State(state): State<ProxyServerState>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    handle_passthrough_request(
        state,
        headers,
        body,
        AppType::Codex,
        endpoint_with_query(&uri, "/responses/compact"),
    )
    .await
}

fn endpoint_with_query(uri: &Uri, endpoint: &str) -> String {
    match uri.query() {
        Some(query) => format!("{endpoint}?{query}"),
        None => endpoint.to_string(),
    }
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
        Ok(forwarder) => forwarder
            .with_optimizer_config(context.optimizer_config.clone())
            .with_session(context.session_id.clone(), context.session_client_provided),
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
                let upstream_is_sse = is_sse_response(&response);
                if should_use_claude_transform_streaming(
                    is_stream,
                    upstream_is_sse,
                    api_format,
                    forward_result.provider.is_codex_oauth(),
                ) {
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
    let api_format = super::providers::get_claude_api_format(provider);
    let response_result = if adapter.needs_transform(provider) {
        build_buffered_claude_transform_response(
            status,
            &response.headers,
            response.body,
            provider.is_codex_oauth() && api_format == "openai_responses",
            |body| adapter.transform_response(body),
        )
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

fn build_buffered_claude_transform_response<F>(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: Bytes,
    aggregate_codex_oauth_responses_sse: bool,
    transform: F,
) -> Result<PreparedResponse, ProxyError>
where
    F: FnOnce(Value) -> Result<Value, ProxyError>,
{
    if aggregate_codex_oauth_responses_sse {
        let body = responses_sse_to_response_value(&String::from_utf8_lossy(&body))?;
        let body = serde_json::to_vec(&body).map_err(|error| {
            ProxyError::RequestFailed(format!("serialize aggregated upstream SSE failed: {error}"))
        })?;
        return build_buffered_json_response(status, headers, Bytes::from(body), transform);
    }

    build_buffered_json_response(status, headers, body, transform)
}

fn responses_sse_to_response_value(body: &str) -> Result<Value, ProxyError> {
    let mut buffer = body.to_string();
    let mut completed_response: Option<Value> = None;
    let mut output_items = Vec::new();

    while let Some(block) = take_sse_block(&mut buffer) {
        let mut event_name = "";
        let mut data_lines = Vec::new();

        for line in block.lines() {
            if let Some(event) = strip_sse_field(line, "event") {
                event_name = event.trim();
            } else if let Some(data) = strip_sse_field(line, "data") {
                data_lines.push(data);
            }
        }

        if data_lines.is_empty() {
            continue;
        }

        let data = data_lines.join("\n");
        if data.trim() == "[DONE]" {
            continue;
        }

        let data: Value = serde_json::from_str(&data).map_err(|error| {
            ProxyError::TransformError(format!("Failed to parse upstream SSE event: {error}"))
        })?;

        match event_name {
            "response.output_item.done" => {
                if let Some(item) = data.get("item") {
                    output_items.push(item.clone());
                }
            }
            "response.completed" => {
                completed_response = Some(data.get("response").cloned().unwrap_or(data));
            }
            "response.failed" => {
                let message = data
                    .pointer("/response/error/message")
                    .and_then(|value| value.as_str())
                    .unwrap_or("response.failed event received");
                return Err(ProxyError::TransformError(message.to_string()));
            }
            _ => {}
        }
    }

    let mut response = completed_response.ok_or_else(|| {
        ProxyError::TransformError("No response.completed event in upstream SSE".to_string())
    })?;

    if !output_items.is_empty() {
        let Some(response) = response.as_object_mut() else {
            return Err(ProxyError::TransformError(
                "response.completed payload is not an object".to_string(),
            ));
        };
        response.insert("output".to_string(), Value::Array(output_items));
    }

    Ok(response)
}

fn should_use_claude_transform_streaming(
    requested_streaming: bool,
    upstream_is_sse: bool,
    api_format: &str,
    is_codex_oauth: bool,
) -> bool {
    requested_streaming || upstream_is_sse || (is_codex_oauth && api_format == "openai_responses")
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
        Ok(forwarder) => forwarder
            .with_optimizer_config(context.optimizer_config.clone())
            .with_session(context.session_id.clone(), context.session_client_provided)
            .with_codex_chat_history(context.state.codex_chat_history.clone()),
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
        let converts_codex_chat = super::providers::should_convert_codex_responses_to_chat(
            &forward_result.provider,
            &endpoint,
        );
        let success_sync = status.is_success().then(|| SuccessSyncInfo {
            app_type: context.app_type.clone(),
            provider: forward_result.provider.clone(),
            current_provider_id_at_start: context.current_provider_id_at_start.clone(),
        });
        let response_result = match response {
            super::forwarder::StreamingResponse::Live(response)
                if converts_codex_chat && status.is_success() =>
            {
                build_codex_chat_stream_response(
                    response,
                    remaining_timeout(first_byte_timeout, request_started_at),
                    context.streaming_idle_timeout(),
                    context.state.codex_chat_history.clone(),
                )
            }
            super::forwarder::StreamingResponse::Live(response) if converts_codex_chat => {
                build_codex_chat_error_response(
                    response,
                    remaining_timeout(first_byte_timeout, request_started_at),
                    context.state.codex_chat_history.clone(),
                )
                .await
            }
            super::forwarder::StreamingResponse::Live(response) => {
                build_passthrough_response(
                    response,
                    remaining_timeout(first_byte_timeout, request_started_at),
                    context.streaming_idle_timeout(),
                )
                .await
            }
            super::forwarder::StreamingResponse::Buffered(response) if converts_codex_chat => {
                build_buffered_codex_chat_response(
                    status,
                    &response.headers,
                    response.body,
                    context.state.codex_chat_history.clone(),
                )
                .await
            }
            super::forwarder::StreamingResponse::Buffered(response) => {
                build_buffered_passthrough_response(status, &response.headers, response.body)
            }
        };
        let request_log = converts_codex_chat.then(|| {
            RequestLogContext::from_handler(
                &context,
                forward_result.provider.clone(),
                true,
                UsageLogPolicy::Transformed,
            )
        });
        return ResponseHandler::finish_streaming(
            &context.state,
            response_result,
            status,
            success_sync,
            request_log,
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
    let converts_codex_chat = super::providers::should_convert_codex_responses_to_chat(
        &forward_result.provider,
        &endpoint,
    );
    let success_sync = response.status.is_success().then(|| SuccessSyncInfo {
        app_type: context.app_type.clone(),
        provider: forward_result.provider.clone(),
        current_provider_id_at_start: context.current_provider_id_at_start.clone(),
    });
    let request_log = converts_codex_chat.then(|| {
        RequestLogContext::from_handler(
            &context,
            forward_result.provider.clone(),
            false,
            UsageLogPolicy::Transformed,
        )
    });
    let status = response.status;
    let response_result = if converts_codex_chat {
        build_buffered_codex_chat_response(
            response.status,
            &response.headers,
            response.body,
            context.state.codex_chat_history.clone(),
        )
        .await
    } else {
        build_buffered_passthrough_response(response.status, &response.headers, response.body)
    };
    ResponseHandler::finish_buffered(
        &context.state,
        response_result,
        status,
        success_sync,
        request_log,
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

#[cfg(test)]
mod tests {
    use super::{
        build_buffered_claude_transform_response, endpoint_with_query,
        responses_sse_to_response_value, should_use_claude_transform_streaming,
    };
    use crate::proxy::error::ProxyError;
    use axum::http::Uri;
    use bytes::Bytes;
    use serde_json::Value;

    #[test]
    fn codex_endpoint_with_query_preserves_responses_query() {
        let uri: Uri = "/v1/responses?foo=bar&api-version=2025-01-01"
            .parse()
            .unwrap();

        assert_eq!(
            endpoint_with_query(&uri, "/responses"),
            "/responses?foo=bar&api-version=2025-01-01"
        );
    }

    #[test]
    fn codex_endpoint_with_query_preserves_responses_compact_query() {
        let uri: Uri = "/v1/responses/compact?foo=bar".parse().unwrap();

        assert_eq!(
            endpoint_with_query(&uri, "/responses/compact"),
            "/responses/compact?foo=bar"
        );
    }

    #[test]
    fn codex_oauth_buffered_transform_aggregates_sse_before_json_parse() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("text/event-stream"),
        );
        let sse = r#"event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp_1","status":"completed","model":"gpt-5.4","output":[],"usage":{"input_tokens":10,"output_tokens":2}}}

"#;

        let prepared = build_buffered_claude_transform_response(
            reqwest::StatusCode::OK,
            &headers,
            Bytes::from(sse),
            true,
            Ok,
        )
        .unwrap();
        let body: Value = serde_json::from_slice(
            prepared
                .body_bytes
                .as_ref()
                .expect("buffered response should keep body bytes"),
        )
        .unwrap();

        assert_eq!(body["id"], "resp_1");
        assert_eq!(body["output"][0]["type"], "message");
        assert_eq!(body["output"][0]["content"][0]["text"], "hello");
    }

    #[test]
    fn codex_oauth_buffered_transform_aggregates_sse_without_content_type() {
        let headers = reqwest::header::HeaderMap::new();
        let sse = r#"event: response.completed
data: {"type":"response.completed","response":{"id":"resp_1","status":"completed","model":"gpt-5.4","output":[],"usage":{"input_tokens":10,"output_tokens":2}}}

"#;

        let prepared = build_buffered_claude_transform_response(
            reqwest::StatusCode::OK,
            &headers,
            Bytes::from(sse),
            true,
            Ok,
        )
        .unwrap();
        let body: Value = serde_json::from_slice(
            prepared
                .body_bytes
                .as_ref()
                .expect("buffered response should keep body bytes"),
        )
        .unwrap();

        assert_eq!(body["id"], "resp_1");
    }

    #[test]
    fn codex_oauth_responses_force_streaming_even_without_sse_content_type() {
        assert!(should_use_claude_transform_streaming(
            false,
            false,
            "openai_responses",
            true,
        ));
    }

    #[test]
    fn upstream_sse_response_always_uses_streaming_path() {
        assert!(should_use_claude_transform_streaming(
            false,
            true,
            "openai_chat",
            false,
        ));
    }

    #[test]
    fn non_streaming_response_stays_non_streaming_for_regular_openai_responses() {
        assert!(!should_use_claude_transform_streaming(
            false,
            false,
            "openai_responses",
            false,
        ));
    }

    #[test]
    fn responses_sse_to_response_value_collects_output_items() {
        let sse = r#"event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp_1","status":"completed","model":"gpt-5.4","output":[],"usage":{"input_tokens":10,"output_tokens":2}}}

"#;

        let response = responses_sse_to_response_value(sse).unwrap();

        assert_eq!(response["id"], "resp_1");
        assert_eq!(response["output"][0]["type"], "message");
        assert_eq!(response["output"][0]["content"][0]["text"], "hello");
    }

    #[test]
    fn responses_sse_to_response_value_handles_crlf_delimiters() {
        let sse = "event: response.output_item.done\r\n\
data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}}\r\n\
\r\n\
event: response.completed\r\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_crlf\",\"status\":\"completed\",\"model\":\"gpt-5.4\",\"output\":[],\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\r\n\
\r\n";

        let response = responses_sse_to_response_value(sse).unwrap();

        assert_eq!(response["id"], "resp_crlf");
        assert_eq!(response["output"][0]["type"], "message");
        assert_eq!(response["output"][0]["content"][0]["text"], "hi");
    }

    #[test]
    fn responses_sse_to_response_value_returns_err_on_response_failed() {
        let sse = "event: response.failed\n\
data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"upstream blew up\"}}}\n\n";

        let err = responses_sse_to_response_value(sse).unwrap_err();
        match err {
            ProxyError::TransformError(message) => assert!(message.contains("upstream blew up")),
            other => panic!("expected TransformError, got {other:?}"),
        }
    }

    #[test]
    fn responses_sse_to_response_value_errors_when_no_completed_event() {
        let sse = "event: response.output_item.done\n\
data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\"}}\n\n";

        assert!(responses_sse_to_response_value(sse).is_err());
    }
}
