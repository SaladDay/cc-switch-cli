use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use cc_switch_lib::{Database, Provider, ProviderMeta, ProxyService};
use serde_json::{json, Map, Value};
use serial_test::serial;
use tokio::sync::Mutex;

async fn bind_test_listener() -> tokio::net::TcpListener {
    let mut last_error = None;
    for _ in 0..20 {
        match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => return listener,
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
    }

    panic!(
        "bind upstream listener: {:?}",
        last_error.expect("listener bind should produce an error")
    );
}

#[derive(Clone, Default)]
struct UpstreamState {
    request_body: Arc<Mutex<Option<Value>>>,
}

#[derive(Clone, Copy)]
enum UpstreamFormat {
    OpenAiChat,
    Anthropic,
}

struct MappingCase {
    provider_env: Map<String, Value>,
    request_body: Value,
    expected_model: &'static str,
    upstream_format: UpstreamFormat,
}

async fn handle_openai_chat(
    State(state): State<UpstreamState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    *state.request_body.lock().await = Some(body);

    (
        StatusCode::OK,
        Json(json!({
            "id": "chatcmpl-model-mapping",
            "object": "chat.completion",
            "created": 123,
            "model": "gpt-4o-mini",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "ok"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 1,
                "completion_tokens": 1,
                "total_tokens": 2
            }
        })),
    )
}

async fn handle_anthropic_messages(
    State(state): State<UpstreamState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    *state.request_body.lock().await = Some(body.clone());

    (
        StatusCode::OK,
        Json(json!({
            "id": "msg_model_mapping",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "text",
                "text": "ok"
            }],
            "model": body.get("model").cloned().unwrap_or_else(|| json!("")),
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 1,
                "output_tokens": 1
            }
        })),
    )
}

async fn assert_forwarded_model(case: MappingCase) {
    let upstream_state = UpstreamState::default();
    let upstream_router = match case.upstream_format {
        UpstreamFormat::OpenAiChat => Router::new()
            .route("/v1/chat/completions", post(handle_openai_chat))
            .with_state(upstream_state.clone()),
        UpstreamFormat::Anthropic => Router::new()
            .route("/v1/messages", post(handle_anthropic_messages))
            .with_state(upstream_state.clone()),
    };

    let upstream_listener = bind_test_listener().await;
    let upstream_addr = upstream_listener
        .local_addr()
        .expect("read upstream address");
    let upstream_handle = tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream_router).await;
    });

    let db = Arc::new(Database::memory().expect("create memory database"));
    let mut env = case.provider_env;
    env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        json!(format!("http://{}", upstream_addr)),
    );
    env.insert("ANTHROPIC_API_KEY".to_string(), json!("sk-test-claude"));

    let provider = Provider {
        id: "claude-model-mapping".to_string(),
        name: "Claude Model Mapping".to_string(),
        settings_config: json!({ "env": env }),
        website_url: None,
        category: Some("claude".to_string()),
        created_at: None,
        sort_index: None,
        notes: None,
        meta: match case.upstream_format {
            UpstreamFormat::OpenAiChat => Some(ProviderMeta {
                api_format: Some("openai_chat".to_string()),
                ..ProviderMeta::default()
            }),
            UpstreamFormat::Anthropic => None,
        },
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    };
    db.save_provider("claude", &provider)
        .expect("save test provider");
    db.set_current_provider("claude", &provider.id)
        .expect("set current provider");

    db.set_app_proxy_preferred_port("claude", 0)
        .expect("update claude app proxy port");

    let service = ProxyService::new(db);
    let mut config = service.get_config().await.expect("read proxy config");
    config.listen_port = 0;
    let proxy = service
        .start_with_runtime_config(config)
        .await
        .expect("start proxy service");
    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "http://{}:{}/v1/messages",
            proxy.address, proxy.port
        ))
        .header("anthropic-version", "2023-06-01")
        .json(&case.request_body)
        .send()
        .await
        .expect("send request to proxy");

    assert!(
        response.status().is_success(),
        "proxy should forward the request successfully"
    );

    let upstream_body = upstream_state
        .request_body
        .lock()
        .await
        .clone()
        .expect("upstream should receive request body");
    assert_eq!(
        upstream_body.get("model").and_then(|value| value.as_str()),
        Some(case.expected_model)
    );

    service.stop().await.expect("stop proxy service");
    upstream_handle.abort();
}

fn anthropic_request(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 64,
        "messages": [{
            "role": "user",
            "content": "hello"
        }]
    })
}

#[tokio::test]
#[serial]
async fn haiku_request_uses_haiku_model_override() {
    let mut provider_env = Map::new();
    provider_env.insert(
        "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
        json!("haiku-mapped"),
    );

    assert_forwarded_model(MappingCase {
        provider_env,
        request_body: anthropic_request("claude-3-5-haiku-20241022"),
        expected_model: "haiku-mapped",
        upstream_format: UpstreamFormat::OpenAiChat,
    })
    .await;
}

#[tokio::test]
#[serial]
async fn sonnet_request_uses_sonnet_model_override() {
    let mut provider_env = Map::new();
    provider_env.insert(
        "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
        json!("sonnet-mapped"),
    );

    assert_forwarded_model(MappingCase {
        provider_env,
        request_body: anthropic_request("claude-3-7-sonnet-20250219"),
        expected_model: "sonnet-mapped",
        upstream_format: UpstreamFormat::OpenAiChat,
    })
    .await;
}

#[tokio::test]
#[serial]
async fn opus_request_uses_opus_model_override() {
    let mut provider_env = Map::new();
    provider_env.insert(
        "ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(),
        json!("opus-mapped"),
    );

    assert_forwarded_model(MappingCase {
        provider_env,
        request_body: anthropic_request("claude-3-opus-20240229"),
        expected_model: "opus-mapped",
        upstream_format: UpstreamFormat::OpenAiChat,
    })
    .await;
}

#[tokio::test]
#[serial]
async fn thinking_enabled_ignores_legacy_reasoning_model_override() {
    let mut provider_env = Map::new();
    provider_env.insert(
        "ANTHROPIC_REASONING_MODEL".to_string(),
        json!("reasoning-mapped"),
    );
    let mut request_body = anthropic_request("claude-3-7-sonnet-20250219");
    request_body["thinking"] = json!({ "type": "enabled" });

    assert_forwarded_model(MappingCase {
        provider_env,
        request_body,
        expected_model: "claude-3-7-sonnet-20250219",
        upstream_format: UpstreamFormat::OpenAiChat,
    })
    .await;
}

#[tokio::test]
#[serial]
async fn thinking_adaptive_ignores_legacy_reasoning_model_override() {
    let mut provider_env = Map::new();
    provider_env.insert(
        "ANTHROPIC_REASONING_MODEL".to_string(),
        json!("reasoning-mapped"),
    );
    let mut request_body = anthropic_request("claude-3-7-sonnet-20250219");
    request_body["thinking"] = json!({ "type": "adaptive" });

    assert_forwarded_model(MappingCase {
        provider_env,
        request_body,
        expected_model: "claude-3-7-sonnet-20250219",
        upstream_format: UpstreamFormat::OpenAiChat,
    })
    .await;
}

#[tokio::test]
#[serial]
async fn reasoning_only_config_keeps_non_thinking_request_model() {
    let mut provider_env = Map::new();
    provider_env.insert(
        "ANTHROPIC_REASONING_MODEL".to_string(),
        json!("reasoning-mapped"),
    );

    assert_forwarded_model(MappingCase {
        provider_env,
        request_body: anthropic_request("claude-3-7-sonnet-20250219"),
        expected_model: "claude-3-7-sonnet-20250219",
        upstream_format: UpstreamFormat::OpenAiChat,
    })
    .await;
}

#[tokio::test]
#[serial]
async fn unknown_thinking_type_treats_request_as_disabled() {
    let mut provider_env = Map::new();
    provider_env.insert(
        "ANTHROPIC_REASONING_MODEL".to_string(),
        json!("reasoning-mapped"),
    );
    provider_env.insert(
        "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
        json!("sonnet-mapped"),
    );
    let mut request_body = anthropic_request("claude-3-7-sonnet-20250219");
    request_body["thinking"] = json!({ "type": "experimental" });

    assert_forwarded_model(MappingCase {
        provider_env,
        request_body,
        expected_model: "sonnet-mapped",
        upstream_format: UpstreamFormat::OpenAiChat,
    })
    .await;
}

#[tokio::test]
#[serial]
async fn model_matching_is_case_insensitive() {
    let mut provider_env = Map::new();
    provider_env.insert(
        "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
        json!("sonnet-mapped"),
    );

    assert_forwarded_model(MappingCase {
        provider_env,
        request_body: anthropic_request("Claude-3-7-SONNET-20250219"),
        expected_model: "sonnet-mapped",
        upstream_format: UpstreamFormat::OpenAiChat,
    })
    .await;
}

#[tokio::test]
#[serial]
async fn default_model_fallback_uses_anthropic_model_override() {
    let mut provider_env = Map::new();
    provider_env.insert("ANTHROPIC_MODEL".to_string(), json!("default-mapped"));

    assert_forwarded_model(MappingCase {
        provider_env,
        request_body: anthropic_request("claude-unknown-model"),
        expected_model: "default-mapped",
        upstream_format: UpstreamFormat::OpenAiChat,
    })
    .await;
}

#[tokio::test]
#[serial]
async fn no_mapping_config_keeps_original_model() {
    assert_forwarded_model(MappingCase {
        provider_env: Map::new(),
        request_body: anthropic_request("claude-3-7-sonnet-20250219"),
        expected_model: "claude-3-7-sonnet-20250219",
        upstream_format: UpstreamFormat::OpenAiChat,
    })
    .await;
}

#[tokio::test]
#[serial]
async fn anthropic_direct_path_uses_mapped_model() {
    let mut provider_env = Map::new();
    provider_env.insert(
        "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
        json!("sonnet-mapped"),
    );

    assert_forwarded_model(MappingCase {
        provider_env,
        request_body: anthropic_request("claude-3-7-sonnet-20250219"),
        expected_model: "sonnet-mapped",
        upstream_format: UpstreamFormat::Anthropic,
    })
    .await;
}
