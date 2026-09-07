use super::{start_model_fetch_system, AppType, ModelFetchMsg, ModelFetchReq};
use crate::cli::tui::form::ProviderAddField;
use axum::{
    http::{HeaderMap, StatusCode, Uri},
    Router,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn model_fetch_worker_channels_preserve_app_identity_overrides_and_host_errors() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let handler_observed = Arc::clone(&observed);
    let router = Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
        let observed = Arc::clone(&handler_observed);
        async move {
            let denied = uri.path().starts_with("/denied/");
            observed.lock().unwrap().push((uri.to_string(), headers));
            if denied {
                (StatusCode::FORBIDDEN, "denied")
            } else {
                (
                    StatusCode::OK,
                    r#"{"data":[{"id":"a"},{"id":"b"},{"id":"a"}]}"#,
                )
            }
        }
    });
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = runtime.spawn(async move { axum::serve(listener, router).await.unwrap() });
    let system = start_model_fetch_system().unwrap();

    // Destination fields are deliberately independent of App identity here.
    // These are channel contract fixtures, not full-product compatibility evidence.
    let cases = [
        (AppType::Claude, None, "/v1/models", true, true, false),
        (AppType::Gemini, None, "/models", false, false, true),
        (AppType::Codex, None, "/models", true, false, false),
        (AppType::OpenCode, None, "/models", true, false, false),
        (AppType::OpenClaw, None, "/models", true, false, false),
        (AppType::Hermes, None, "/models", true, false, false),
        (AppType::Pi, None, "/models", true, false, false),
        (
            AppType::Pi,
            Some("anthropic-messages"),
            "/v1/models",
            true,
            true,
            false,
        ),
        (
            AppType::Claude,
            Some("google-generative-ai"),
            "/models",
            false,
            false,
            true,
        ),
        (
            AppType::Gemini,
            Some("unknown"),
            "/models",
            false,
            false,
            true,
        ),
    ];
    for (index, (app_type, protocol, path, bearer, anthropic, google)) in
        cases.into_iter().enumerate()
    {
        let field = if index % 2 == 0 {
            ProviderAddField::CodexLocalRouting
        } else {
            ProviderAddField::OpenClawModels
        };
        let request_id = index as u64 + 1;
        system
            .req_tx
            .send(ModelFetchReq::Fetch {
                request_id,
                app_type,
                base_url: origin.clone(),
                is_full_url: false,
                api_key: Some(" fake-key ".into()),
                custom_user_agent: Some("channel-test/1".into()),
                api_protocol: protocol.map(str::to_owned),
                request_headers: Some([("x-host-header".into(), "retained".into())].into()),
                codex_oauth: false,
                codex_oauth_account_id: None,
                field,
                claude_idx: Some(index),
            })
            .unwrap();
        let ModelFetchMsg::Finished {
            request_id: actual_id,
            field: actual_field,
            claude_idx,
            result,
        } = system
            .result_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert_eq!(
            (actual_id, actual_field, claude_idx),
            (request_id, field, Some(index))
        );
        assert_eq!(result.unwrap(), ["a", "b"]);
        let requests = observed.lock().unwrap();
        assert_eq!(requests.len(), index + 1);
        assert_eq!(requests[index].0, path);
        let headers = &requests[index].1;
        for (name, expected) in [
            ("authorization", bearer.then_some("Bearer fake-key")),
            ("x-api-key", anthropic.then_some("fake-key")),
            ("anthropic-version", anthropic.then_some("2023-06-01")),
            ("x-goog-api-key", google.then_some("fake-key")),
        ] {
            assert_eq!(
                headers.get(name).map(|v| v.to_str().unwrap()),
                expected,
                "case {index}: {name}"
            );
        }
        assert_eq!(headers["user-agent"], "channel-test/1");
        assert_eq!(headers["x-host-header"], "retained");
    }

    for (index, key, expected) in [
        (
            11,
            None,
            "API Key or request headers are required to fetch models".to_string(),
        ),
        (
            12,
            Some("fake-key"),
            format!("HTTP 403 Forbidden ({origin}/denied/models)"),
        ),
    ] {
        system
            .req_tx
            .send(ModelFetchReq::Fetch {
                request_id: index,
                app_type: AppType::Gemini,
                base_url: format!("{origin}/denied"),
                is_full_url: false,
                api_key: key.map(str::to_owned),
                custom_user_agent: None,
                api_protocol: None,
                request_headers: None,
                codex_oauth: false,
                codex_oauth_account_id: None,
                field: ProviderAddField::OpenClawModels,
                claude_idx: None,
            })
            .unwrap();
        let ModelFetchMsg::Finished {
            request_id,
            field,
            claude_idx,
            result,
        } = system
            .result_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert_eq!(
            (request_id, field, claude_idx),
            (index, ProviderAddField::OpenClawModels, None)
        );
        assert_eq!(result, Err(expected));
        assert_eq!(observed.lock().unwrap().len(), index as usize - 1);
    }
    drop(system.req_tx);
    system._handle.join().unwrap();
    server.abort();
    let _ = runtime.block_on(server);
}
