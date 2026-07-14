use axum::{body::to_bytes, http::StatusCode};
use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde_json::json;
use std::{io::Write, sync::Arc};

use super::*;

async fn buffered_body(response: Response) -> Bytes {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read buffered response body")
}

#[test]
fn decode_buffered_response_body_decompresses_gzip_and_strips_entity_headers() {
    let payload = br#"{"ok":true}"#;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(payload).expect("write gzip payload");
    let compressed = encoder.finish().expect("finish gzip payload");
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::CONTENT_ENCODING,
        HeaderValue::from_static("gzip"),
    );
    headers.insert(
        reqwest::header::CONTENT_LENGTH,
        HeaderValue::from_static("999"),
    );
    headers.insert(
        reqwest::header::TRANSFER_ENCODING,
        HeaderValue::from_static("chunked"),
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let body = decode_buffered_response_body(&mut headers, Bytes::from(compressed));

    assert_eq!(body, Bytes::from_static(payload));
    assert!(!headers.contains_key(reqwest::header::CONTENT_ENCODING));
    assert!(!headers.contains_key(reqwest::header::CONTENT_LENGTH));
    assert!(!headers.contains_key(reqwest::header::TRANSFER_ENCODING));
    assert_eq!(
        headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
}

#[tokio::test]
async fn non_success_parse_failures_fall_back_to_upstream_response() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let prepared = build_buffered_json_response(
        reqwest::StatusCode::BAD_REQUEST,
        &headers,
        Bytes::from_static(br#"{not-json"#),
        |_| Ok(json!({"type": "error"})),
    )
    .expect("fallback to raw upstream response");

    assert_eq!(prepared.response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        prepared
            .response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    assert_eq!(
        buffered_body(prepared.response).await,
        Bytes::from_static(br#"{not-json"#)
    );
}

#[tokio::test]
async fn non_success_transform_failures_fall_back_to_upstream_response() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let prepared = build_buffered_json_response(
        reqwest::StatusCode::BAD_REQUEST,
        &headers,
        Bytes::from_static(br#"{"message":"upstream rejected the request"}"#),
        |_| {
            Err(ProxyError::TransformError(
                "missing error envelope".to_string(),
            ))
        },
    )
    .expect("fallback to raw upstream response");

    assert_eq!(prepared.response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        buffered_body(prepared.response).await,
        Bytes::from_static(br#"{"message":"upstream rejected the request"}"#)
    );
}

#[test]
fn non_success_non_transform_failures_preserve_original_proxy_error() {
    let headers = HeaderMap::new();
    let result = build_buffered_json_response(
        reqwest::StatusCode::BAD_REQUEST,
        &headers,
        Bytes::from_static(br#"{"message":"upstream rejected the request"}"#),
        |_| {
            Err(ProxyError::Timeout(
                "proxy transform pipeline broke".to_string(),
            ))
        },
    );

    match result {
        Ok(_) => panic!("non-transform errors must not fall back to upstream passthrough"),
        Err(ProxyError::Timeout(message)) => {
            assert_eq!(message, "proxy transform pipeline broke");
        }
        Err(other) => panic!("expected original proxy error, got {other:?}"),
    }
}

#[test]
fn success_parse_failures_use_proxy_request_failed_errors() {
    let headers = HeaderMap::new();
    let result = build_buffered_json_response(
        reqwest::StatusCode::OK,
        &headers,
        Bytes::from_static(br#"{not-json"#),
        |_| Ok(json!({"type": "message"})),
    );

    match result {
        Ok(_) => panic!("success responses should still fail on malformed upstream json"),
        Err(ProxyError::RequestFailed(message)) => {
            assert!(message.contains("parse upstream json failed"));
        }
        Err(other) => panic!("expected request failed error, got {other:?}"),
    }
}

#[test]
fn success_transform_failures_use_proxy_request_failed_errors() {
    let headers = HeaderMap::new();
    let result = build_buffered_json_response(
        reqwest::StatusCode::OK,
        &headers,
        Bytes::from_static(br#"{"message":"upstream accepted the request"}"#),
        |_| {
            Err(ProxyError::TransformError(
                "missing success envelope".to_string(),
            ))
        },
    );

    match result {
        Ok(_) => panic!("success responses must surface transform failures as proxy errors"),
        Err(ProxyError::RequestFailed(message)) => {
            assert!(message.contains("transform upstream json failed"));
            assert!(message.contains("missing success envelope"));
        }
        Err(other) => panic!("expected request failed error, got {other:?}"),
    }
}

#[tokio::test]
async fn non_success_standard_json_errors_can_still_transform() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let prepared = build_buffered_json_response(
        reqwest::StatusCode::BAD_REQUEST,
        &headers,
        Bytes::from_static(
            br#"{"error":{"message":"upstream rejected the request","type":"invalid_request_error"}}"#,
        ),
        |body| {
            assert_eq!(
                body,
                json!({
                    "error": {
                        "message": "upstream rejected the request",
                        "type": "invalid_request_error"
                    }
                })
            );
            Ok(json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": "upstream rejected the request"
                }
            }))
        },
    )
    .expect("standard upstream json errors should still transform");

    assert_eq!(prepared.response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value =
        serde_json::from_slice(&buffered_body(prepared.response).await).expect("response json");
    assert_eq!(
        body,
        json!({
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "upstream rejected the request"
            }
        })
    );
}

#[tokio::test]
async fn codex_chat_buffered_success_converts_to_responses_shape() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let prepared = build_buffered_codex_chat_response(
        reqwest::StatusCode::OK,
        &headers,
        Bytes::from_static(
            br#"{"id":"chatcmpl_123","object":"chat.completion","created":1710000000,"model":"deepseek-chat","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}"#,
        ),
        Arc::new(Default::default()),
    )
    .await
    .expect("convert Chat success response");

    assert_eq!(prepared.response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&buffered_body(prepared.response).await).expect("response json");
    assert_eq!(body["object"], "response");
    assert_eq!(body["model"], "deepseek-chat");
    assert_eq!(body["output"][0]["type"], "message");
    assert_eq!(body["output"][0]["content"][0]["text"], "hello");
    assert_eq!(body["usage"]["input_tokens"], 3);
    assert_eq!(body["usage"]["output_tokens"], 2);
}

#[tokio::test]
async fn codex_chat_buffered_response_restores_tool_context_identity() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let request = json!({
        "model": "gpt-5.4",
        "tools": [{"type": "tool_search"}],
        "input": "Find Gmail tools"
    });
    let tool_context = transform_codex_chat::build_codex_tool_context_from_request(&request);

    let prepared = build_buffered_codex_chat_response_with_context(
        reqwest::StatusCode::OK,
        &headers,
        Bytes::from_static(
            br#"{"id":"chatcmpl_tool_search","object":"chat.completion","created":1710000000,"model":"gpt-5.4","choices":[{"index":0,"message":{"role":"assistant","tool_calls":[{"id":"call_tool_search_1","type":"function","function":{"name":"tool_search","arguments":"{\"query\":\"Gmail search emails\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        ),
        Arc::new(Default::default()),
        tool_context,
    )
    .await
    .expect("convert Chat tool_search response");

    let body: serde_json::Value =
        serde_json::from_slice(&buffered_body(prepared.response).await).expect("response json");
    assert_eq!(body["output"][0]["type"], "tool_search_call");
    assert_eq!(body["output"][0]["call_id"], "call_tool_search_1");
    assert_eq!(
        body["output"][0]["arguments"]["query"],
        "Gmail search emails"
    );
}

#[tokio::test]
async fn codex_chat_buffered_transform_strips_hop_by_hop_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        reqwest::header::CONNECTION,
        HeaderValue::from_static("x-trace-hop, keep-alive"),
    );
    headers.insert(
        reqwest::header::HeaderName::from_static("x-trace-hop"),
        HeaderValue::from_static("trace"),
    );
    headers.insert(
        reqwest::header::HeaderName::from_static("keep-alive"),
        HeaderValue::from_static("timeout=5"),
    );
    headers.insert(
        reqwest::header::HeaderName::from_static("proxy-connection"),
        HeaderValue::from_static("keep-alive"),
    );
    headers.insert(
        reqwest::header::UPGRADE,
        HeaderValue::from_static("websocket"),
    );
    headers.insert(
        reqwest::header::CONTENT_ENCODING,
        HeaderValue::from_static("gzip"),
    );
    headers.insert("x-stable-header", HeaderValue::from_static("kept"));

    let prepared = build_buffered_codex_chat_response(
        reqwest::StatusCode::OK,
        &headers,
        Bytes::from_static(
            br#"{"id":"chatcmpl_123","object":"chat.completion","created":1710000000,"model":"deepseek-chat","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]}"#,
        ),
        Arc::new(Default::default()),
    )
    .await
    .expect("convert Chat success response");

    let response_headers = prepared.response.headers();
    assert!(!response_headers.contains_key(reqwest::header::CONNECTION));
    assert!(!response_headers.contains_key("x-trace-hop"));
    assert!(!response_headers.contains_key("keep-alive"));
    assert!(!response_headers.contains_key("proxy-connection"));
    assert!(!response_headers.contains_key(reqwest::header::UPGRADE));
    assert!(!response_headers.contains_key(reqwest::header::CONTENT_ENCODING));
    assert_eq!(
        response_headers
            .get("x-stable-header")
            .and_then(|value| value.to_str().ok()),
        Some("kept")
    );
}

#[test]
fn buffered_passthrough_strips_hop_by_hop_headers() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        reqwest::header::CONNECTION,
        HeaderValue::from_static("x-trace-hop, upgrade"),
    );
    headers.insert(
        reqwest::header::HeaderName::from_static("x-trace-hop"),
        HeaderValue::from_static("trace"),
    );
    headers.insert(
        reqwest::header::HeaderName::from_static("proxy-connection"),
        HeaderValue::from_static("keep-alive"),
    );
    headers.insert(
        reqwest::header::UPGRADE,
        HeaderValue::from_static("websocket"),
    );
    headers.insert(
        reqwest::header::CONTENT_ENCODING,
        HeaderValue::from_static("gzip"),
    );
    headers.insert("x-stable-header", HeaderValue::from_static("kept"));

    let prepared = build_buffered_passthrough_response(
        reqwest::StatusCode::OK,
        &headers,
        Bytes::from_static(br#"{"ok":true}"#),
    )
    .expect("build passthrough response");

    let response_headers = prepared.response.headers();
    assert!(!response_headers.contains_key(reqwest::header::CONNECTION));
    assert!(!response_headers.contains_key("x-trace-hop"));
    assert!(!response_headers.contains_key("proxy-connection"));
    assert!(!response_headers.contains_key(reqwest::header::UPGRADE));
    assert_eq!(
        response_headers
            .get(reqwest::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("gzip")
    );
    assert_eq!(
        response_headers
            .get("x-stable-header")
            .and_then(|value| value.to_str().ok()),
        Some("kept")
    );
}

#[tokio::test]
async fn codex_chat_buffered_error_converts_non_json_body_to_responses_error() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));

    let prepared = build_buffered_codex_chat_response(
        reqwest::StatusCode::BAD_GATEWAY,
        &headers,
        Bytes::from_static(b"upstream unavailable"),
        Arc::new(Default::default()),
    )
    .await
    .expect("convert Chat error response");

    assert_eq!(prepared.response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        prepared
            .response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body: serde_json::Value =
        serde_json::from_slice(&buffered_body(prepared.response).await).expect("response json");
    assert_eq!(body["error"]["message"], "upstream unavailable");
    assert_eq!(body["error"]["type"], "upstream_error");
}
