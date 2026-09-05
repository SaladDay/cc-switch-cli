use super::*;
use axum::{
    http::{HeaderMap, StatusCode, Uri},
    Router,
};
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

// Keep the previous protocol discriminator only in the independent test oracle.
#[derive(Debug, Clone, Copy)]
enum ModelFetchStrategy {
    Bearer,
    Anthropic,
    GoogleApiKey,
}

impl ModelFetchStrategy {
    fn spec(self) -> &'static cc_switch_core::model_fetch::ModelFetchSpec {
        use cc_switch_core::model_fetch::{
            ANTHROPIC_COMPATIBLE, BEARER_COMPATIBLE, GOOGLE_API_KEY,
        };
        match self {
            Self::Bearer => &BEARER_COMPATIBLE,
            Self::Anthropic => &ANTHROPIC_COMPATIBLE,
            Self::GoogleApiKey => &GOOGLE_API_KEY,
        }
    }
}

#[test]
fn registry_defaults_and_protocol_overrides_keep_tui_app_behavior() {
    use cc_switch_core::model_fetch::{ANTHROPIC_COMPATIBLE, BEARER_COMPATIBLE, GOOGLE_API_KEY};
    for (app, default) in [
        (AppType::Claude, &ANTHROPIC_COMPATIBLE),
        (AppType::Gemini, &GOOGLE_API_KEY),
        (AppType::Codex, &BEARER_COMPATIBLE),
        (AppType::OpenCode, &BEARER_COMPATIBLE),
        (AppType::OpenClaw, &BEARER_COMPATIBLE),
        (AppType::Hermes, &BEARER_COMPATIBLE),
        (AppType::Pi, &BEARER_COMPATIBLE),
    ] {
        for (protocol, expected) in [
            (None, default),
            (Some("anthropic-messages"), &ANTHROPIC_COMPATIBLE),
            (Some("google-generative-ai"), &GOOGLE_API_KEY),
            (Some("openai-completions"), default),
            (Some("unknown"), default),
            (Some(" anthropic-messages "), default),
            (Some(""), default),
        ] {
            assert_eq!(
                model_fetch_spec_for_app(&app, protocol),
                expected,
                "{app:?}, {protocol:?}"
            );
        }
    }
}

#[tokio::test]
async fn real_http_executor_accepts_custom_declarations_without_a_protocol_enum() {
    use cc_switch_core::model_fetch::{
        ModelEndpointPolicy, ModelFetchSpec, ModelHeaderValue, ModelListShape,
    };
    let spec = ModelFetchSpec {
        endpoints: ModelEndpointPolicy::VersionedFirst {
            compatibility_suffixes: &[],
        },
        key_headers: &[
            ("x-native-key", ModelHeaderValue::Key { prefix: "Token " }),
            ("x-native-version", ModelHeaderValue::Literal("2")),
        ],
        response_shapes: &[ModelListShape {
            collection_pointer: "/result/catalog",
            id_pointer: "/native/id",
            strip_prefix: Some("model/"),
        }],
    };
    let observed = Arc::new(Mutex::new(Vec::new()));
    let handler_observed = Arc::clone(&observed);
    let router = Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
        let observed = Arc::clone(&handler_observed);
        async move {
            observed.lock().unwrap().push((uri.to_string(), headers));
            axum::Json(json!({"result": {"catalog": [
                {"native": {"id": "model/a"}, "capabilities": {"future": true}},
                {"native": {"id": "model/a"}}, {"native": {"id": "model/b"}}
            ]}, "cursor": "opaque"}))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let result =
        fetch_provider_models_for_tui(&origin, false, Some(" fake-key "), None, &spec, None).await;
    server.abort();
    let _ = server.await;
    assert_eq!(result.unwrap(), ["a", "b"]);
    let requests = observed.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "/v1/models");
    let headers = &requests[0].1;
    assert_eq!(headers["x-native-key"], "Token fake-key");
    assert_eq!(headers["x-native-version"], "2");
    for absent in [
        "authorization",
        "x-api-key",
        "x-goog-api-key",
        "anthropic-version",
    ] {
        assert!(!headers.contains_key(absent), "{absent}");
    }
}

#[test]
fn candidates_and_response_ids_match_the_previous_production_implementation() {
    let roots = [
        "",
        "/",
        "not a URL",
        "https://relay.example",
        "http://127.0.0.1:9",
        "https://雪.example",
    ];
    let paths = [
        "",
        "/v1",
        "/v1beta",
        "/models",
        "/anthropic",
        "/API/CLAUDECODE",
        "/apps/anthropic",
        "/api/coding",
        "/step_plan",
        "/v1/messages?version=1",
        "/custom/chat/completions",
        "/models?key=x",
        "/v1/",
        "/coding///",
    ];
    for strategy in [
        ModelFetchStrategy::Bearer,
        ModelFetchStrategy::Anthropic,
        ModelFetchStrategy::GoogleApiKey,
    ] {
        for root in roots {
            for path in paths {
                for full in [false, true] {
                    for url in [format!("{root}{path}"), format!(" \t{root}{path}/ \n")] {
                        assert_eq!(
                            build_model_fetch_candidate_urls(&url, strategy.spec(), full),
                            baseline_candidate_urls(&url, strategy, full),
                            "{url:?}, {strategy:?}, full={full}"
                        );
                    }
                }
            }
        }
        let items = [
            json!([]),
            json!([null, {}, {"id": 7}]),
            json!([{"id": ""}, {"id": "a"}, {"id": "a"}]),
            json!([{"name": "models/a"}, {"name": "models/models/b"}, {"name": "models/"}]),
            json!([{"id": " 雪 ", "name": "models/alternate"}, {"id": []}, {"id": "b"}]),
        ];
        for data in &items {
            for models in &items {
                for payload in [
                    json!({"data": data, "models": models, "has_more": true}),
                    data.clone(),
                    models.clone(),
                ] {
                    assert_eq!(
                        strategy.spec().parse_model_ids(&payload),
                        baseline_model_ids(&payload)
                    );
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObservedRequest {
    path: String,
    headers: BTreeMap<String, Vec<Vec<u8>>>,
}

#[derive(Default)]
struct Responses {
    script: Vec<(StatusCode, &'static str)>,
    observed: Vec<ObservedRequest>,
}

#[tokio::test]
async fn real_http_requests_results_and_failures_match_the_baseline() {
    let state = Arc::new(Mutex::new(Responses::default()));
    let handler_state = Arc::clone(&state);
    let router = Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
        let state = Arc::clone(&handler_state);
        async move {
            let mut state = state.lock().unwrap();
            let response = state
                .script
                .get(state.observed.len())
                .copied()
                .unwrap_or((StatusCode::INTERNAL_SERVER_ERROR, "unexpected request"));
            let headers = headers
                .keys()
                .map(|name| {
                    (
                        name.to_string(),
                        headers
                            .get_all(name)
                            .iter()
                            .map(|value| value.as_bytes().to_vec())
                            .collect(),
                    )
                })
                .collect();
            state.observed.push(ObservedRequest {
                path: uri.to_string(),
                headers,
            });
            (
                response.0,
                [("content-type", "application/json")],
                response.1,
            )
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let scripts = [
        vec![(
            StatusCode::OK,
            r#"{"data":[{"id":"a"},{"id":"a"},{"id":"b"}],"has_more":true}"#,
        )],
        vec![
            (StatusCode::NOT_FOUND, ""),
            (StatusCode::OK, r#"{"models":[{"name":"models/a"}]}"#),
        ],
        vec![
            (StatusCode::METHOD_NOT_ALLOWED, ""),
            (StatusCode::OK, r#"[{"id":"a"}]"#),
        ],
        vec![
            (StatusCode::OK, "not json"),
            (StatusCode::OK, r#"{"data":[{"id":""}]}"#),
        ],
        vec![
            (StatusCode::OK, "{}"),
            (StatusCode::OK, r#"{"data":[{"id":"a"}]}"#),
        ],
        vec![
            (StatusCode::SERVICE_UNAVAILABLE, ""),
            (StatusCode::OK, r#"{"data":[{"id":"not reached"}]}"#),
        ],
        vec![(StatusCode::NOT_FOUND, ""), (StatusCode::NOT_FOUND, "")],
        vec![(StatusCode::OK, "{}"), (StatusCode::OK, "{}")],
    ];
    let custom = BTreeMap::from([
        ("Authorization".into(), "Custom auth".into()),
        ("x-api-key".into(), "Custom key".into()),
        ("x-extension".into(), "keep".into()),
        ("User-Agent".into(), "custom-header-agent".into()),
    ]);
    for strategy in [
        ModelFetchStrategy::Bearer,
        ModelFetchStrategy::Anthropic,
        ModelFetchStrategy::GoogleApiKey,
    ] {
        for script in &scripts {
            for key in [None, Some(" key "), Some("bad\r\nkey")] {
                for full in [false, true] {
                    let url = if full {
                        format!("{origin}/v1/messages?version=1")
                    } else {
                        origin.clone()
                    };
                    *state.lock().unwrap() = Responses {
                        script: script.clone(),
                        observed: Vec::new(),
                    };
                    let expected =
                        baseline_fetch(&url, full, key, Some(" agent "), strategy, Some(&custom))
                            .await;
                    let expected_requests = state.lock().unwrap().observed.clone();
                    state.lock().unwrap().observed.clear();
                    let actual = fetch_provider_models_for_tui(
                        &url,
                        full,
                        key,
                        Some(" agent "),
                        strategy.spec(),
                        Some(&custom),
                    )
                    .await;
                    assert_eq!(
                        actual, expected,
                        "{strategy:?}, full={full}, script={script:?}"
                    );
                    assert_eq!(
                        state.lock().unwrap().observed,
                        expected_requests,
                        "{strategy:?}, full={full}"
                    );
                }
            }
        }
    }
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn preflight_errors_match_without_sending_requests() {
    let excessive = (0..65)
        .map(|n| (format!("x-{n}"), "v".to_string()))
        .collect();
    let invalid_name = BTreeMap::from([("invalid name".into(), "v".into())]);
    let invalid_value = BTreeMap::from([("x-field".into(), "bad\nvalue".into())]);
    for (url, full, key, headers) in [
        ("", false, None, None),
        ("https://relay.invalid", true, None, None),
        ("http://127.0.0.1:9", false, None, None),
        ("http://127.0.0.1:9", false, Some("key"), Some(&excessive)),
        (
            "http://127.0.0.1:9",
            false,
            Some("key"),
            Some(&invalid_name),
        ),
        (
            "http://127.0.0.1:9",
            false,
            Some("key"),
            Some(&invalid_value),
        ),
    ] {
        let expected =
            baseline_fetch(url, full, key, None, ModelFetchStrategy::Anthropic, headers).await;
        let actual = fetch_provider_models_for_tui(
            url,
            full,
            key,
            None,
            ModelFetchStrategy::Anthropic.spec(),
            headers,
        )
        .await;
        assert!(expected.is_err());
        assert_eq!(actual, expected);
    }
}

// Test-only functions copied from CLI 739ac4a3, with local names to keep the oracle independent.
const KNOWN_COMPAT_SUFFIXES: &[&str] = &[
    "/api/claudecode",
    "/api/anthropic",
    "/apps/anthropic",
    "/api/coding",
    "/claudecode",
    "/anthropic",
    "/step_plan",
    "/coding",
    "/claude",
];

fn baseline_candidate_urls(
    base_url: &str,
    strategy: ModelFetchStrategy,
    is_full_url: bool,
) -> Vec<String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Vec::new();
    }

    if is_full_url {
        let mut urls = Vec::new();
        if let Some(index) = base.find("/v1/") {
            urls.push(format!("{}/v1/models", &base[..index]));
        } else if let Some(index) = base.rfind('/') {
            let root = &base[..index];
            if root
                .find("://")
                .is_some_and(|scheme| root.len() > scheme.saturating_add(3))
            {
                urls.push(format!("{root}/v1/models"));
            }
        }
        return urls;
    }

    if base.ends_with("/models") {
        return vec![base.to_string()];
    }

    let append_models = format!("{base}/models");
    let append_versioned_models = if base.ends_with("/v1") || base.ends_with("/v1beta") {
        None
    } else {
        Some(format!("{base}/v1/models"))
    };

    let mut urls: Vec<String> = Vec::new();
    match strategy {
        ModelFetchStrategy::Anthropic => {
            if let Some(versioned) = append_versioned_models.as_ref() {
                urls.push(versioned.clone());
            } else {
                urls.push(append_models.clone());
            }

            if let Some(stripped) = strip_compat_suffix(base) {
                let root = stripped.trim_end_matches('/');
                if !root.is_empty() && root.contains("://") {
                    urls.push(format!("{root}/v1/models"));
                    urls.push(format!("{root}/models"));
                }
            } else if append_versioned_models.is_some() {
                urls.push(append_models);
            }
        }
        ModelFetchStrategy::Bearer | ModelFetchStrategy::GoogleApiKey => {
            urls.push(append_models);
            if let Some(v1) = append_versioned_models.as_ref() {
                urls.push(v1.clone());
            }
        }
    }

    let mut seen = HashSet::new();
    urls.retain(|url| seen.insert(url.clone()));
    urls
}

fn strip_compat_suffix(base: &str) -> Option<&str> {
    let lower = base.to_ascii_lowercase();
    KNOWN_COMPAT_SUFFIXES.iter().find_map(|suffix| {
        lower
            .ends_with(suffix)
            .then(|| &base[..base.len() - suffix.len()])
    })
}

fn baseline_model_ids(payload: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    if let Some(data) = payload.get("data").and_then(|v| v.as_array()) {
        for item in data {
            if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                out.push(id.to_string());
            }
        }
    }

    if out.is_empty() {
        if let Some(models) = payload.get("models").and_then(|v| v.as_array()) {
            for item in models {
                if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                    out.push(name.strip_prefix("models/").unwrap_or(name).to_string());
                }
            }
        }
    }

    if out.is_empty() {
        if let Some(arr) = payload.as_array() {
            for item in arr {
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                    out.push(id.to_string());
                }
            }
        }
    }

    let mut seen = HashSet::new();
    out.retain(|model| seen.insert(model.clone()));
    out
}

async fn baseline_fetch(
    base_url: &str,
    is_full_url: bool,
    api_key: Option<&str>,
    custom_user_agent: Option<&str>,
    strategy: ModelFetchStrategy,
    request_headers: Option<&BTreeMap<String, String>>,
) -> Result<Vec<String>, String> {
    let candidate_urls = baseline_candidate_urls(base_url, strategy, is_full_url);
    if candidate_urls.is_empty() {
        return Err(if is_full_url && !base_url.trim().is_empty() {
            "Cannot derive models endpoint from full URL".to_string()
        } else {
            "URL cannot be empty".to_string()
        });
    }

    let client = crate::proxy::http_client::get();

    let key = api_key.map(str::trim).filter(|k| !k.is_empty());
    let custom_user_agent = crate::provider::parse_custom_user_agent(custom_user_agent)
        .ok()
        .flatten();
    if key.is_none() && request_headers.is_none_or(BTreeMap::is_empty) {
        return Err("API Key or request headers are required to fetch models".to_string());
    }
    if request_headers.is_some_and(|headers| headers.len() > 64) {
        return Err("Too many model-fetch request headers (maximum 64)".to_string());
    }
    let mut last_err = String::from("unknown error");

    for url in candidate_urls {
        let mut req = client.get(&url).timeout(Duration::from_secs(5));
        if let Some(key) = key {
            req = match strategy {
                ModelFetchStrategy::Bearer => req.header("Authorization", format!("Bearer {key}")),
                ModelFetchStrategy::Anthropic => req
                    .header("Authorization", format!("Bearer {key}"))
                    .header("x-api-key", key)
                    .header("anthropic-version", "2023-06-01"),
                ModelFetchStrategy::GoogleApiKey => req.header("x-goog-api-key", key),
            };
        }
        if let Some(user_agent) = &custom_user_agent {
            req = req.header(reqwest::header::USER_AGENT, user_agent.clone());
        }
        if let Some(request_headers) = request_headers {
            for (raw_name, raw_value) in request_headers {
                let name = reqwest::header::HeaderName::from_bytes(raw_name.trim().as_bytes())
                    .map_err(|error| {
                        format!("Invalid model-fetch header name {raw_name}: {error}")
                    })?;
                let value = reqwest::header::HeaderValue::from_str(raw_value).map_err(|error| {
                    format!("Invalid model-fetch header value for {name}: {error}")
                })?;
                req = req.header(name, value);
            }
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if !status.is_success() {
                    last_err = format!("HTTP {status} ({url})");
                    if status != reqwest::StatusCode::NOT_FOUND
                        && status != reqwest::StatusCode::METHOD_NOT_ALLOWED
                    {
                        return Err(last_err);
                    }
                    continue;
                }
                match resp.json::<Value>().await {
                    Ok(payload) => {
                        let models = baseline_model_ids(&payload);
                        if models.is_empty() {
                            last_err = format!("No model list found in response ({url})");
                        } else {
                            return Ok(models);
                        }
                    }
                    Err(err) => {
                        last_err = format!("Invalid JSON response ({url}): {err}");
                    }
                }
            }
            Err(err) => {
                last_err = err.to_string();
            }
        }
    }

    Err(last_err)
}
