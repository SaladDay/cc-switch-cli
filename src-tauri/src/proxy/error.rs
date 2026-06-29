use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum ProxyError {
    #[error("proxy server is already running")]
    AlreadyRunning,

    #[error("proxy server is not running")]
    NotRunning,

    #[error("proxy bind failed: {0}")]
    BindFailed(String),

    #[error("proxy stop timed out")]
    StopTimeout,

    #[error("proxy stop failed: {0}")]
    StopFailed(String),

    #[error("proxy forward failed: {0}")]
    ForwardFailed(String),

    #[error("no available provider")]
    NoAvailableProvider,

    #[error("all providers are circuit open")]
    AllProvidersCircuitOpen,

    #[error("no providers configured")]
    NoProvidersConfigured,

    #[error("provider unhealthy: {0}")]
    ProviderUnhealthy(String),

    #[error("{}", upstream_error_message(*status, body.as_deref()))]
    UpstreamError { status: u16, body: Option<String> },

    #[error("max retries exceeded")]
    MaxRetriesExceeded,

    #[error("proxy database error: {0}")]
    DatabaseError(String),

    #[error("proxy config error: {0}")]
    ConfigError(String),

    #[error("proxy auth error: {0}")]
    AuthError(String),

    #[error("proxy request failed: {0}")]
    RequestFailed(String),

    #[error("proxy transform error: {0}")]
    TransformError(String),

    #[error("invalid proxy request: {0}")]
    InvalidRequest(String),

    #[error("proxy timeout: {0}")]
    Timeout(String),

    #[error("stream idle timeout after {0} seconds")]
    StreamIdleTimeout(u64),

    #[error("proxy internal error: {0}")]
    Internal(String),
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            ProxyError::UpstreamError {
                status: upstream_status,
                body,
            } => {
                let status =
                    StatusCode::from_u16(upstream_status).unwrap_or(StatusCode::BAD_GATEWAY);
                let body = upstream_error_body(upstream_status, body);
                (status, body)
            }
            error => {
                let status = error.status_code();
                let body = json!({
                    "error": {
                        "message": proxy_error_message(error),
                        "type": "proxy_error",
                    }
                });
                (status, body)
            }
        };

        (status, Json(body)).into_response()
    }
}

impl ProxyError {
    pub fn status_code(&self) -> StatusCode {
        proxy_error_status(self)
    }
}

fn proxy_error_status(error: &ProxyError) -> StatusCode {
    match error {
        ProxyError::AlreadyRunning => StatusCode::CONFLICT,
        ProxyError::NotRunning => StatusCode::SERVICE_UNAVAILABLE,
        ProxyError::BindFailed(_)
        | ProxyError::StopTimeout
        | ProxyError::StopFailed(_)
        | ProxyError::DatabaseError(_)
        | ProxyError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        ProxyError::ForwardFailed(_) | ProxyError::RequestFailed(_) => StatusCode::BAD_GATEWAY,
        ProxyError::NoAvailableProvider
        | ProxyError::AllProvidersCircuitOpen
        | ProxyError::NoProvidersConfigured
        | ProxyError::ProviderUnhealthy(_)
        | ProxyError::MaxRetriesExceeded => StatusCode::SERVICE_UNAVAILABLE,
        ProxyError::ConfigError(_) | ProxyError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
        ProxyError::AuthError(_) => StatusCode::UNAUTHORIZED,
        ProxyError::TransformError(_) => StatusCode::UNPROCESSABLE_ENTITY,
        ProxyError::Timeout(_) | ProxyError::StreamIdleTimeout(_) => StatusCode::GATEWAY_TIMEOUT,
        ProxyError::UpstreamError { status, .. } => {
            StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY)
        }
    }
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

fn upstream_error_message(status: u16, body: Option<&str>) -> String {
    match summarize_upstream_error_body(body) {
        Some(summary) => format!("upstream returned {status}: {summary}"),
        None => format!("upstream returned {status}"),
    }
}

fn summarize_upstream_error_body(body: Option<&str>) -> Option<String> {
    let body = body?.trim();
    if body.is_empty() {
        return None;
    }

    if let Ok(json_body) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(message) = extract_json_error_message(&json_body) {
            return Some(summarize_text_for_log(&message, 180));
        }

        if let Ok(compact_json) = serde_json::to_string(&json_body) {
            return Some(summarize_text_for_log(&compact_json, 180));
        }
    }

    Some(summarize_text_for_log(body, 180))
}

fn upstream_error_body(status: u16, body: Option<String>) -> serde_json::Value {
    match body {
        Some(body) => serde_json::from_str::<serde_json::Value>(&body).unwrap_or_else(|_| {
            json!({
                "error": {
                    "message": body,
                    "type": "upstream_error",
                }
            })
        }),
        None => json!({
            "error": {
                "message": format!("Upstream error (status {status})"),
                "type": "upstream_error",
            }
        }),
    }
}

fn extract_json_error_message(body: &serde_json::Value) -> Option<String> {
    [
        body.pointer("/error/message"),
        body.pointer("/message"),
        body.pointer("/detail"),
        body.pointer("/error"),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| value.as_str().map(ToString::to_string))
}

fn summarize_text_for_log(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = normalized.trim();

    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }

    let truncated: String = trimmed.chars().take(max_chars).collect();
    format!("{}...", truncated.trim_end())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ErrorCategory {
    Retryable,
    NonRetryable,
    ClientAbort,
}

#[allow(dead_code)]
pub fn categorize_error(error: &reqwest::Error) -> ErrorCategory {
    if error.is_timeout() || error.is_connect() {
        return ErrorCategory::Retryable;
    }

    if let Some(status) = error.status() {
        if status.is_server_error() {
            ErrorCategory::Retryable
        } else if status.is_client_error() {
            ErrorCategory::NonRetryable
        } else {
            ErrorCategory::Retryable
        }
    } else {
        ErrorCategory::Retryable
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::to_bytes, http::StatusCode, response::IntoResponse, routing::get, Json, Router,
    };
    use serde_json::{json, Value};

    use super::*;

    async fn bind_listener() -> tokio::net::TcpListener {
        tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener")
    }

    #[tokio::test]
    async fn upstream_error_passthroughs_json_body() {
        let response = ProxyError::UpstreamError {
            status: 429,
            body: Some(
                r#"{"error":{"message":"rate limited","type":"rate_limit_error"}}"#.to_string(),
            ),
        }
        .into_response();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        let body: Value = serde_json::from_slice(&body).expect("parse json body");
        assert_eq!(
            body,
            json!({"error": {"message": "rate limited", "type": "rate_limit_error"}})
        );
    }

    #[tokio::test]
    async fn upstream_error_wraps_plain_text_body_in_upstream_error_shape() {
        let response = ProxyError::UpstreamError {
            status: 502,
            body: Some("bad gateway".to_string()),
        }
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        let body: Value = serde_json::from_slice(&body).expect("parse json body");
        assert_eq!(
            body,
            json!({"error": {"message": "bad gateway", "type": "upstream_error"}})
        );
    }

    #[test]
    fn upstream_error_display_summarizes_json_body_for_logs() {
        let error = ProxyError::UpstreamError {
            status: 429,
            body: Some(
                r#"{"error":{"message":"rate limit exceeded for workspace alpha"}}"#.to_string(),
            ),
        };

        assert_eq!(
            error.to_string(),
            "upstream returned 429: rate limit exceeded for workspace alpha"
        );
    }

    #[tokio::test]
    async fn no_available_provider_maps_to_service_unavailable() {
        let response = ProxyError::NoAvailableProvider.into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        let body: Value = serde_json::from_slice(&body).expect("parse json body");
        assert_eq!(
            body,
            json!({"error": {"message": "no available provider", "type": "proxy_error"}})
        );
    }

    #[tokio::test]
    async fn request_failed_uses_nested_proxy_error_shape() {
        let response =
            ProxyError::RequestFailed("request timed out after 1s".to_string()).into_response();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        let body: Value = serde_json::from_slice(&body).expect("parse json body");
        assert_eq!(
            body,
            json!({"error": {"message": "request timed out after 1s", "type": "proxy_error"}})
        );
    }

    #[test]
    fn proxy_error_status_mappings_align_with_upstream() {
        let cases = [
            (ProxyError::AlreadyRunning, StatusCode::CONFLICT),
            (ProxyError::NotRunning, StatusCode::SERVICE_UNAVAILABLE),
            (
                ProxyError::BindFailed("bind failed".to_string()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (ProxyError::StopTimeout, StatusCode::INTERNAL_SERVER_ERROR),
            (
                ProxyError::StopFailed("stop failed".to_string()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                ProxyError::ForwardFailed("forward failed".to_string()),
                StatusCode::BAD_GATEWAY,
            ),
            (
                ProxyError::NoAvailableProvider,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                ProxyError::AllProvidersCircuitOpen,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                ProxyError::NoProvidersConfigured,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                ProxyError::ProviderUnhealthy("unhealthy".to_string()),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                ProxyError::MaxRetriesExceeded,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                ProxyError::DatabaseError("db failed".to_string()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                ProxyError::ConfigError("bad config".to_string()),
                StatusCode::BAD_REQUEST,
            ),
            (
                ProxyError::AuthError("bad auth".to_string()),
                StatusCode::UNAUTHORIZED,
            ),
            (
                ProxyError::RequestFailed("send failed".to_string()),
                StatusCode::BAD_GATEWAY,
            ),
            (
                ProxyError::TransformError("bad body".to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                ProxyError::InvalidRequest("bad request".to_string()),
                StatusCode::BAD_REQUEST,
            ),
            (
                ProxyError::Timeout("timed out".to_string()),
                StatusCode::GATEWAY_TIMEOUT,
            ),
            (
                ProxyError::StreamIdleTimeout(30),
                StatusCode::GATEWAY_TIMEOUT,
            ),
            (
                ProxyError::Internal("boom".to_string()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];

        for (error, expected_status) in cases {
            assert_eq!(error.into_response().status(), expected_status);
        }
    }

    #[tokio::test]
    async fn categorize_error_marks_4xx_as_non_retryable_and_5xx_as_retryable() {
        async fn bad_request() -> (StatusCode, Json<Value>) {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "bad request"})),
            )
        }

        async fn server_error() -> (StatusCode, Json<Value>) {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "service unavailable"})),
            )
        }

        let router = Router::new()
            .route("/bad-request", get(bad_request))
            .route("/server-error", get(server_error));
        let listener = bind_listener().await;
        let addr = listener.local_addr().expect("listener addr");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        let client = reqwest::Client::new();
        let bad_request_error = client
            .get(format!("http://{addr}/bad-request"))
            .send()
            .await
            .expect("send bad request")
            .error_for_status()
            .expect_err("4xx should produce reqwest error");
        assert_eq!(
            categorize_error(&bad_request_error),
            ErrorCategory::NonRetryable
        );

        let server_error = client
            .get(format!("http://{addr}/server-error"))
            .send()
            .await
            .expect("send server error")
            .error_for_status()
            .expect_err("5xx should produce reqwest error");
        assert_eq!(categorize_error(&server_error), ErrorCategory::Retryable);

        server.abort();
    }
}
