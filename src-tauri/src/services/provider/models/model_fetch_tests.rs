use super::ProviderService;
use crate::error::AppError;
use axum::{
    http::{HeaderMap, StatusCode, Uri},
    Router,
};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn public_model_fetch_keeps_response_request_and_localized_error_contracts() {
    // Expectations come from the public API at CLI 875854da, not the TUI API.
    let observed = Arc::new(Mutex::new(Vec::new()));
    let response = Arc::new(Mutex::new((StatusCode::OK, String::new())));
    let handler_observed = Arc::clone(&observed);
    let handler_response = Arc::clone(&response);
    let router = Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
        let observed = Arc::clone(&handler_observed);
        let response = Arc::clone(&handler_response);
        async move {
            observed.lock().unwrap().push((uri.to_string(), headers));
            response.lock().unwrap().clone()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    for (payload, expected) in [
        (
            r#"{"data":[{"id":"a"},{"id":"b"},{"id":"a"}]}"#,
            vec!["a", "b"],
        ),
        (
            r#"{"data":[{"id":""},{"id":null},{"id":" a "},{"id":"雪"}],"models":[{"name":"ignored"}]}"#,
            vec!["", " a ", "雪"],
        ),
        (
            r#"{"data":[{"id":7}],"models":[{"name":"models/models/a"},{"name":"models/"},{"name":"models/models/a"}]}"#,
            vec!["models/a", ""],
        ),
        (r#"[{"id":"a"},null,{"id":"b"},{"id":"a"}]"#, vec!["a", "b"]),
    ] {
        for key in [None, Some(""), Some(" fake-key ")] {
            observed.lock().unwrap().clear();
            *response.lock().unwrap() = (StatusCode::OK, payload.into());
            let actual = ProviderService::fetch_provider_models(&origin, key)
                .await
                .unwrap();
            assert_eq!(actual, expected);
            let requests = observed.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].0, "/models");
            let headers = &requests[0].1;
            assert_eq!(
                headers.get("authorization").map(|v| v.to_str().unwrap()),
                key.map(|k| format!("Bearer {}", k.trim()))
                    .as_deref()
                    .map(str::trim_end)
            );
            assert_eq!(
                headers.get("x-api-key").map(|v| v.to_str().unwrap()),
                key.map(str::trim)
            );
            assert!(!headers.contains_key("anthropic-version"));
            assert!(!headers.contains_key("x-goog-api-key"));
        }
    }

    for (status, payload, paths, zh, en) in [
        (
            StatusCode::OK,
            "{}",
            vec!["/models", "/v1/models"],
            "未能在响应中找到模型列表",
            "No model list found in response",
        ),
        (
            StatusCode::OK,
            "not-json",
            vec!["/models", "/v1/models"],
            "无法解析 JSON 响应",
            "Failed to parse JSON response",
        ),
        (
            StatusCode::NOT_FOUND,
            "",
            vec!["/models", "/v1/models"],
            "HTTP 404 Not Found",
            "HTTP 404 Not Found",
        ),
        (
            StatusCode::METHOD_NOT_ALLOWED,
            "",
            vec!["/models", "/v1/models"],
            "HTTP 405 Method Not Allowed",
            "HTTP 405 Method Not Allowed",
        ),
        (
            StatusCode::FORBIDDEN,
            "",
            vec!["/models"],
            "HTTP 403 Forbidden",
            "HTTP 403 Forbidden",
        ),
    ] {
        observed.lock().unwrap().clear();
        *response.lock().unwrap() = (status, payload.into());
        let error = ProviderService::fetch_provider_models(&origin, None)
            .await
            .unwrap_err();
        let url = format!("{origin}{}", paths.last().unwrap());
        assert!(
            matches!(error, AppError::Localized { key: "fetch.failed", zh: actual_zh, en: actual_en }
            if actual_zh == format!("拉取失败: {zh} (URL: {url})")
                && actual_en == format!("Fetch failed: {en} (URL: {url})"))
        );
        assert_eq!(
            observed
                .lock()
                .unwrap()
                .iter()
                .map(|r| r.0.clone())
                .collect::<Vec<_>>(),
            paths
        );
    }
    observed.lock().unwrap().clear();
    let error = ProviderService::fetch_provider_models(" \t/", None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, AppError::Localized { key: "fetch.invalid_url", zh, en }
        if zh == "URL 不能为空" && en == "URL cannot be empty")
    );
    assert!(observed.lock().unwrap().is_empty());
    server.abort();
    let _ = server.await;
}
