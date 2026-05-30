use axum::{body::Body, http, response::Response};
use bytes::Bytes;
use futures::stream::StreamExt;
use serde_json::Value;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

mod error_summary;
#[cfg(test)]
mod tests;

use self::error_summary::{summarize_upstream_body_bytes, summarize_upstream_json_value};
use super::{
    error::ProxyError,
    metrics::estimate_tokens_from_bytes,
    providers::{
        codex_chat_history::{record_responses_sse_stream, CodexChatHistoryStore},
        streaming::create_anthropic_sse_stream,
        streaming_codex_chat::create_responses_sse_stream_from_chat,
        streaming_responses::create_anthropic_sse_stream_from_responses,
        transform_codex_chat,
    },
};

pub struct PreparedResponse {
    pub response: Response,
    pub stream_completion: Option<StreamCompletion>,
    pub estimated_output_tokens: u64,
    pub upstream_error_summary: Option<String>,
    pub body_bytes: Option<Bytes>,
}

impl PreparedResponse {
    fn buffered(
        response: Response,
        estimated_output_tokens: u64,
        upstream_error_summary: Option<String>,
        body_bytes: Bytes,
    ) -> Self {
        Self {
            response,
            stream_completion: None,
            estimated_output_tokens,
            upstream_error_summary,
            body_bytes: Some(body_bytes),
        }
    }

    fn streaming(response: Response, stream_completion: StreamCompletion) -> Self {
        Self {
            response,
            stream_completion: Some(stream_completion),
            estimated_output_tokens: 0,
            upstream_error_summary: None,
            body_bytes: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct StreamCompletion {
    inner: Arc<Mutex<Option<Result<(), String>>>>,
}

impl StreamCompletion {
    pub fn record_success(&self) {
        let mut outcome = self.inner.lock().expect("lock stream completion");
        if outcome.is_none() {
            *outcome = Some(Ok(()));
        }
    }

    pub fn record_error(&self, message: String) {
        let mut outcome = self.inner.lock().expect("lock stream completion");
        if outcome.is_none() {
            *outcome = Some(Err(message));
        }
    }

    pub fn outcome(&self) -> Option<Result<(), String>> {
        self.inner.lock().expect("lock stream completion").clone()
    }
}

pub fn is_sse_response(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("text/event-stream"))
        .unwrap_or(false)
}

pub async fn build_passthrough_response(
    response: reqwest::Response,
    first_byte_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
) -> Result<PreparedResponse, ProxyError> {
    let status = response.status();
    let headers = response.headers().clone();
    let mut builder = Response::builder().status(status);
    copy_headers(&mut builder, &headers, false);

    if is_sse_response(&response) {
        let stream_completion = StreamCompletion::default();
        let stream = with_stream_timeouts(
            response.bytes_stream(),
            first_byte_timeout,
            idle_timeout,
            Some(stream_completion.clone()),
        );
        return builder
            .body(Body::from_stream(stream))
            .map(|response| PreparedResponse::streaming(response, stream_completion))
            .map_err(|error| {
                ProxyError::RequestFailed(format!("build streaming response failed: {error}"))
            });
    }

    let body = read_buffered_body(response, first_byte_timeout).await?;
    let upstream_error_summary = if !status.is_success() {
        summarize_upstream_body_bytes(&body)
    } else {
        None
    };
    let estimated_output_tokens = estimate_tokens_from_bytes(&body);
    let response_bytes = body.clone();
    builder
        .body(Body::from(body))
        .map(|response| {
            PreparedResponse::buffered(
                response,
                estimated_output_tokens,
                upstream_error_summary,
                response_bytes,
            )
        })
        .map_err(|error| {
            ProxyError::RequestFailed(format!("build passthrough response failed: {error}"))
        })
}

pub async fn build_json_response<F>(
    response: reqwest::Response,
    first_byte_timeout: Option<Duration>,
    transform: F,
) -> Result<PreparedResponse, ProxyError>
where
    F: FnOnce(Value) -> Result<Value, ProxyError>,
{
    let status = response.status();
    let headers = response.headers().clone();
    let body = read_buffered_body(response, first_byte_timeout).await?;
    build_buffered_json_response_inner(status, &headers, body, transform)
}

pub async fn build_codex_chat_error_response(
    response: reqwest::Response,
    first_byte_timeout: Option<Duration>,
    history: Arc<CodexChatHistoryStore>,
) -> Result<PreparedResponse, ProxyError> {
    let status = response.status();
    let headers = response.headers().clone();
    let body = read_buffered_body(response, first_byte_timeout).await?;
    build_buffered_codex_chat_response(status, &headers, body, history).await
}

pub fn build_buffered_passthrough_response(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: Bytes,
) -> Result<PreparedResponse, ProxyError> {
    let upstream_error_summary = if !status.is_success() {
        summarize_upstream_body_bytes(&body)
    } else {
        None
    };
    let estimated_output_tokens = estimate_tokens_from_bytes(&body);
    let mut builder = Response::builder().status(status);
    copy_headers(&mut builder, headers, false);
    let response_bytes = body.clone();
    builder
        .body(Body::from(body))
        .map(|response| {
            PreparedResponse::buffered(
                response,
                estimated_output_tokens,
                upstream_error_summary,
                response_bytes,
            )
        })
        .map_err(|error| {
            ProxyError::RequestFailed(format!("build passthrough response failed: {error}"))
        })
}

pub fn build_buffered_json_response<F>(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: Bytes,
    transform: F,
) -> Result<PreparedResponse, ProxyError>
where
    F: FnOnce(Value) -> Result<Value, ProxyError>,
{
    build_buffered_json_response_inner(status, headers, body, transform)
}

pub fn build_anthropic_stream_response(
    response: reqwest::Response,
    first_byte_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
    api_format: &str,
) -> Result<PreparedResponse, ProxyError> {
    let status = response.status();
    let headers = response.headers().clone();
    let mut builder = Response::builder().status(status);
    copy_headers(&mut builder, &headers, true);

    let stream_completion = StreamCompletion::default();
    let timed_stream = with_stream_timeouts(
        response.bytes_stream(),
        first_byte_timeout,
        idle_timeout,
        None,
    );
    let stream: std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<Bytes, std::io::Error>> + Send>,
    > = if api_format == "openai_responses" {
        Box::pin(create_anthropic_sse_stream_from_responses(
            timed_stream,
            stream_completion.clone(),
        ))
    } else {
        Box::pin(create_anthropic_sse_stream(
            timed_stream,
            stream_completion.clone(),
        ))
    };
    builder
        .body(Body::from_stream(stream))
        .map(|response| PreparedResponse::streaming(response, stream_completion))
        .map_err(|error| {
            ProxyError::RequestFailed(format!("build anthropic stream response failed: {error}"))
        })
}

pub fn build_codex_chat_stream_response(
    response: reqwest::Response,
    first_byte_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
    history: Arc<CodexChatHistoryStore>,
) -> Result<PreparedResponse, ProxyError> {
    let status = response.status();
    let headers = response.headers().clone();
    let mut builder = Response::builder().status(status);
    copy_headers(&mut builder, &headers, true);

    let stream_completion = StreamCompletion::default();
    let timed_stream = with_stream_timeouts(
        response.bytes_stream(),
        first_byte_timeout,
        idle_timeout,
        Some(stream_completion.clone()),
    );
    let responses_stream = create_responses_sse_stream_from_chat(timed_stream);
    let recorded_stream = record_responses_sse_stream(responses_stream, history);

    builder
        .body(Body::from_stream(recorded_stream))
        .map(|response| PreparedResponse::streaming(response, stream_completion))
        .map_err(|error| {
            ProxyError::RequestFailed(format!("build Codex Chat stream response failed: {error}"))
        })
}

pub async fn build_buffered_codex_chat_response(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: Bytes,
    history: Arc<CodexChatHistoryStore>,
) -> Result<PreparedResponse, ProxyError> {
    let upstream_error_summary = if !status.is_success() {
        summarize_upstream_body_bytes(&body)
    } else {
        None
    };

    let response_body = if status.is_success() {
        let upstream_body: Value = serde_json::from_slice(&body).map_err(|error| {
            ProxyError::RequestFailed(format!("parse upstream chat json failed: {error}"))
        })?;
        let responses_body = transform_codex_chat::chat_completion_to_response(upstream_body)
            .map_err(|error| {
                ProxyError::RequestFailed(format!(
                    "transform upstream chat json failed: {}",
                    proxy_error_message(error)
                ))
            })?;
        history.record_response(&responses_body).await;
        responses_body
    } else {
        let parsed_value = parse_codex_chat_error_body(&body);
        transform_codex_chat::chat_error_to_response_error(Some(&parsed_value))
    };

    let response_body = serde_json::to_vec(&response_body).map_err(|error| {
        ProxyError::RequestFailed(format!("serialize Codex Responses json failed: {error}"))
    })?;
    let response_bytes = Bytes::from(response_body);
    let estimated_output_tokens = estimate_tokens_from_bytes(&response_bytes);

    let mut response_headers = headers.clone();
    response_headers.remove(reqwest::header::CONTENT_TYPE);
    let mut builder = Response::builder().status(status);
    copy_headers(&mut builder, &response_headers, false);
    builder = builder.header("content-type", "application/json");

    builder
        .body(Body::from(response_bytes.clone()))
        .map(|response| {
            PreparedResponse::buffered(
                response,
                estimated_output_tokens,
                upstream_error_summary,
                response_bytes,
            )
        })
        .map_err(|error| {
            ProxyError::RequestFailed(format!("build Codex Responses response failed: {error}"))
        })
}

fn parse_codex_chat_error_body(body: &[u8]) -> Value {
    match serde_json::from_slice::<Value>(body) {
        Ok(value) => value,
        Err(_) => {
            const MAX_RAW_ERROR_BYTES: usize = 1024;
            let lossy = String::from_utf8_lossy(body);
            if lossy.len() <= MAX_RAW_ERROR_BYTES {
                return Value::String(lossy.into_owned());
            }

            let mut end = MAX_RAW_ERROR_BYTES;
            while end > 0 && !lossy.is_char_boundary(end) {
                end -= 1;
            }
            Value::String(format!("{}...(truncated)", &lossy[..end]))
        }
    }
}

fn with_stream_timeouts(
    stream: impl futures::Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    first_byte_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
    stream_completion: Option<StreamCompletion>,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send {
    async_stream::stream! {
        tokio::pin!(stream);
        let mut is_first_chunk = true;

        while let Some(next) = next_chunk_with_timeout(
            &mut stream,
            if is_first_chunk { first_byte_timeout } else { idle_timeout },
            if is_first_chunk {
                StreamTimeoutPhase::FirstByte
            } else {
                StreamTimeoutPhase::Idle
            },
        ).await {
            match next {
                Ok(chunk) => {
                    is_first_chunk = false;
                    yield Ok(chunk);
                }
                Err(error) => {
                    if let Some(stream_completion) = &stream_completion {
                        stream_completion.record_error(error.to_string());
                    }
                    yield Err(error);
                    return;
                }
            }
        }

        if let Some(stream_completion) = &stream_completion {
            stream_completion.record_success();
        }
    }
}

async fn read_buffered_body(
    response: reqwest::Response,
    timeout_duration: Option<Duration>,
) -> Result<Bytes, ProxyError> {
    match timeout_duration {
        Some(timeout) => match tokio::time::timeout(timeout, response.bytes()).await {
            Ok(result) => result.map_err(|error| {
                ProxyError::RequestFailed(format!("read response body failed: {error}"))
            }),
            Err(_) => Err(ProxyError::Timeout(
                StreamTimeoutPhase::FirstByte.error_message(timeout),
            )),
        },
        None => response.bytes().await.map_err(|error| {
            ProxyError::RequestFailed(format!("read response body failed: {error}"))
        }),
    }
}

async fn next_chunk_with_timeout<S>(
    stream: &mut S,
    timeout_duration: Option<Duration>,
    phase: StreamTimeoutPhase,
) -> Option<Result<Bytes, std::io::Error>>
where
    S: futures::Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    let next = match timeout_duration {
        Some(timeout) => match tokio::time::timeout(timeout, stream.next()).await {
            Ok(next) => next,
            Err(_) => {
                return Some(Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    phase.error_message(timeout),
                )));
            }
        },
        None => stream.next().await,
    };

    next.map(|result| result.map_err(std::io::Error::other))
}

#[derive(Clone, Copy)]
enum StreamTimeoutPhase {
    FirstByte,
    Idle,
}

impl StreamTimeoutPhase {
    fn error_message(self, timeout: Duration) -> String {
        let display_seconds = timeout.as_secs().max(u64::from(!timeout.is_zero()));
        match self {
            StreamTimeoutPhase::FirstByte => {
                format!("stream timeout after {}s", display_seconds)
            }
            StreamTimeoutPhase::Idle => {
                format!("stream idle timeout after {}s", display_seconds)
            }
        }
    }
}

fn copy_headers(
    builder: &mut http::response::Builder,
    headers: &reqwest::header::HeaderMap,
    force_sse_content_type: bool,
) {
    for (key, value) in headers {
        let lower = key.as_str().to_ascii_lowercase();
        if lower == "content-length" || lower == "transfer-encoding" {
            continue;
        }
        if force_sse_content_type && lower == "content-type" {
            continue;
        }
        *builder = std::mem::take(builder).header(key, value);
    }

    if force_sse_content_type {
        *builder = std::mem::take(builder).header("content-type", "text/event-stream");
    }
}

fn build_buffered_json_response_inner<F>(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: Bytes,
    transform: F,
) -> Result<PreparedResponse, ProxyError>
where
    F: FnOnce(Value) -> Result<Value, ProxyError>,
{
    let upstream_body: Value = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(_) if !status.is_success() => {
            return build_buffered_passthrough_response(status, headers, body);
        }
        Err(error) => {
            return Err(ProxyError::RequestFailed(format!(
                "parse upstream json failed: {error}"
            )));
        }
    };
    let upstream_error_summary = if !status.is_success() {
        summarize_upstream_json_value(&upstream_body)
    } else {
        None
    };
    let response_body = match transform(upstream_body) {
        Ok(body) => body,
        Err(error) if should_passthrough_transform_failure(status, &error) => {
            return build_buffered_passthrough_response(status, headers, body);
        }
        Err(error) => {
            if !status.is_success() {
                return Err(error);
            }
            return Err(ProxyError::RequestFailed(format!(
                "transform upstream json failed: {}",
                proxy_error_message(error)
            )));
        }
    };
    let response_body = match serde_json::to_vec(&response_body) {
        Ok(body) => body,
        Err(error) => {
            return Err(ProxyError::RequestFailed(format!(
                "serialize transformed json failed: {error}"
            )));
        }
    };
    let response_bytes = Bytes::from(response_body);
    let estimated_output_tokens = estimate_tokens_from_bytes(&response_bytes);

    let mut builder = Response::builder().status(status);
    copy_headers(&mut builder, headers, false);
    builder = builder.header("content-type", "application/json");

    builder
        .body(Body::from(response_bytes.clone()))
        .map(|response| {
            PreparedResponse::buffered(
                response,
                estimated_output_tokens,
                upstream_error_summary,
                response_bytes,
            )
        })
        .map_err(|error| {
            ProxyError::RequestFailed(format!("build transformed response failed: {error}"))
        })
}

fn should_passthrough_transform_failure(status: reqwest::StatusCode, error: &ProxyError) -> bool {
    !status.is_success() && matches!(error, ProxyError::TransformError(_))
}

fn proxy_error_message(error: ProxyError) -> String {
    match error {
        ProxyError::ConfigError(message)
        | ProxyError::AuthError(message)
        | ProxyError::RequestFailed(message)
        | ProxyError::TransformError(message)
        | ProxyError::ForwardFailed(message)
        | ProxyError::BindFailed(message)
        | ProxyError::StopFailed(message)
        | ProxyError::ProviderUnhealthy(message)
        | ProxyError::DatabaseError(message)
        | ProxyError::InvalidRequest(message)
        | ProxyError::Timeout(message)
        | ProxyError::Internal(message) => message,
        other => other.to_string(),
    }
}
