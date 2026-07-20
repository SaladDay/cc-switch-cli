use axum::http::HeaderMap;
use bytes::Bytes;
use futures::{stream::BoxStream, StreamExt};
use serde_json::Value;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{app_config::AppType, provider::Provider};

use super::{
    error::ProxyError,
    provider_router::ProviderRouter,
    providers::codex_chat_history::CodexChatHistoryStore,
    providers::gemini_shadow::GeminiShadowStore,
    providers::get_adapter,
    response::decode_buffered_response_body,
    thinking_budget_rectifier::{rectify_thinking_budget, should_rectify_thinking_budget},
    thinking_rectifier::{
        normalize_thinking_type, rectify_anthropic_request, should_rectify_thinking_signature,
    },
    types::{CopilotOptimizerConfig, OptimizerConfig, RectifierConfig},
};

mod request_builder;

pub struct RequestForwarder {
    router: Arc<ProviderRouter>,
    optimizer_config: OptimizerConfig,
    copilot_optimizer_config: CopilotOptimizerConfig,
    session_id: String,
    session_client_provided: bool,
    codex_chat_history: Option<Arc<CodexChatHistoryStore>>,
    gemini_shadow: Option<Arc<GeminiShadowStore>>,
}

#[derive(Debug, Clone, Copy)]
pub struct ForwardOptions {
    pub max_retries: u32,
    pub request_timeout: Option<Duration>,
    pub bypass_circuit_breaker: bool,
}

#[derive(Debug)]
pub struct BufferedResponse {
    pub status: reqwest::StatusCode,
    pub headers: reqwest::header::HeaderMap,
    pub body: Bytes,
}

#[derive(Debug)]
pub struct ForwardedResponse<T> {
    pub provider: Provider,
    pub response: T,
}

#[derive(Debug)]
pub struct ForwardFailure {
    pub provider: Option<Provider>,
    pub error: ProxyError,
}

impl ForwardFailure {
    fn new(provider: Option<Provider>, error: ProxyError) -> Self {
        Self { provider, error }
    }
}

pub struct LiveResponse {
    status: reqwest::StatusCode,
    headers: reqwest::header::HeaderMap,
    stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
}

impl std::fmt::Debug for LiveResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish_non_exhaustive()
    }
}

impl LiveResponse {
    fn from_reqwest(response: reqwest::Response) -> Self {
        let status = response.status();
        let headers = response.headers().clone();
        Self {
            status,
            headers,
            stream: response.bytes_stream().boxed(),
        }
    }

    fn from_stream(
        status: reqwest::StatusCode,
        headers: reqwest::header::HeaderMap,
        stream: impl futures::Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    ) -> Self {
        Self {
            status,
            headers,
            stream: stream.boxed(),
        }
    }

    pub fn status(&self) -> reqwest::StatusCode {
        self.status
    }

    pub fn headers(&self) -> &reqwest::header::HeaderMap {
        &self.headers
    }

    pub fn bytes_stream(self) -> BoxStream<'static, Result<Bytes, reqwest::Error>> {
        self.stream
    }
}

#[derive(Debug)]
pub enum StreamingResponse {
    Live(LiveResponse),
    Buffered(BufferedResponse),
}

impl StreamingResponse {
    pub fn status(&self) -> reqwest::StatusCode {
        match self {
            Self::Live(response) => response.status(),
            Self::Buffered(response) => response.status,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttemptDecision {
    ProviderFailure,
    NeutralRelease,
    FatalStop,
}

enum BufferedRequestError {
    BeforeResponse(ProxyError),
    AfterResponse(ProxyError),
}

enum StreamingRequestError {
    BeforeResponse(ProxyError),
    AfterResponse(ProxyError),
}

struct BufferedAttemptOutcome {
    response: BufferedResponse,
    attempt_decision: AttemptDecision,
}

struct StreamingAttemptOutcome {
    response: StreamingResponse,
    attempt_decision: AttemptDecision,
}

impl RequestForwarder {
    pub fn new(router: Arc<ProviderRouter>) -> Result<Self, ProxyError> {
        Ok(Self {
            router,
            optimizer_config: OptimizerConfig::default(),
            copilot_optimizer_config: CopilotOptimizerConfig::default(),
            session_id: String::new(),
            session_client_provided: false,
            codex_chat_history: None,
            gemini_shadow: None,
        })
    }

    pub fn with_optimizer_config(mut self, optimizer_config: OptimizerConfig) -> Self {
        self.optimizer_config = optimizer_config;
        self
    }

    pub fn with_copilot_optimizer_config(
        mut self,
        copilot_optimizer_config: CopilotOptimizerConfig,
    ) -> Self {
        self.copilot_optimizer_config = copilot_optimizer_config;
        self
    }

    pub fn with_session(mut self, session_id: String, client_provided: bool) -> Self {
        self.session_id = session_id;
        self.session_client_provided = client_provided;
        self
    }

    pub fn with_codex_chat_history(mut self, history: Arc<CodexChatHistoryStore>) -> Self {
        self.codex_chat_history = Some(history);
        self
    }

    pub fn with_gemini_shadow(mut self, shadow: Arc<GeminiShadowStore>) -> Self {
        self.gemini_shadow = Some(shadow);
        self
    }

    #[cfg(test)]
    #[expect(
        clippy::too_many_arguments,
        reason = "test helper mirrors proxy forwarding inputs"
    )]
    pub async fn forward_response(
        &self,
        app_type: &AppType,
        endpoint: &str,
        body: Value,
        headers: &HeaderMap,
        providers: Vec<Provider>,
        options: ForwardOptions,
        rectifier_config: RectifierConfig,
    ) -> Result<ForwardedResponse<StreamingResponse>, ProxyError> {
        self.forward_response_detailed(
            app_type,
            endpoint,
            body,
            headers,
            providers,
            options,
            rectifier_config,
        )
        .await
        .map_err(|failure| failure.error)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "forwarding requires request, provider, and retry options"
    )]
    pub async fn forward_response_detailed(
        &self,
        app_type: &AppType,
        endpoint: &str,
        body: Value,
        headers: &HeaderMap,
        providers: Vec<Provider>,
        options: ForwardOptions,
        rectifier_config: RectifierConfig,
    ) -> Result<ForwardedResponse<StreamingResponse>, ForwardFailure> {
        if providers.is_empty() {
            return Err(ForwardFailure::new(None, ProxyError::NoAvailableProvider));
        }

        let claude_error_path = matches!(app_type, AppType::Claude);
        let bypass_circuit_breaker = options.bypass_circuit_breaker;
        let mut last_error = None;
        let mut attempted_provider = false;
        let mut attempted_providers = 0usize;
        let mut pending_upstream_response = None;
        let max_attempts = (options.max_retries as usize).saturating_add(1);

        for provider in providers {
            if attempted_providers >= max_attempts {
                break;
            }

            let permit = if bypass_circuit_breaker {
                super::circuit_breaker::AllowResult {
                    allowed: true,
                    used_half_open_permit: false,
                }
            } else {
                self.router
                    .allow_provider_request(&provider.id, app_type.as_str())
                    .await
            };

            if !permit.allowed {
                continue;
            }

            attempted_provider = true;
            attempted_providers += 1;
            pending_upstream_response = None;
            let provider_needs_transform = matches!(app_type, AppType::Claude)
                && get_adapter(app_type).needs_transform(&provider);

            match self
                .send_streaming_request(
                    app_type,
                    &provider,
                    endpoint,
                    &body,
                    headers,
                    ForwardOptions {
                        max_retries: 0,
                        ..options
                    },
                    &rectifier_config,
                )
                .await
            {
                Ok(outcome) => {
                    let response = outcome.response;
                    if response.status().is_success() {
                        if !bypass_circuit_breaker {
                            let _ = self
                                .router
                                .record_result(
                                    &provider.id,
                                    app_type.as_str(),
                                    permit.used_half_open_permit,
                                    true,
                                    None,
                                )
                                .await;
                        }

                        return Ok(ForwardedResponse { provider, response });
                    }

                    match outcome.attempt_decision {
                        AttemptDecision::NeutralRelease => {
                            if !bypass_circuit_breaker {
                                self.router
                                    .release_permit_neutral(
                                        &provider.id,
                                        app_type.as_str(),
                                        permit.used_half_open_permit,
                                    )
                                    .await;
                            }

                            if claude_error_path && !provider_needs_transform {
                                return Err(ForwardFailure::new(
                                    Some(provider),
                                    streaming_response_to_upstream_error(response),
                                ));
                            }

                            return Ok(ForwardedResponse { provider, response });
                        }
                        AttemptDecision::ProviderFailure => {
                            if !bypass_circuit_breaker {
                                let _ = self
                                    .router
                                    .record_result(
                                        &provider.id,
                                        app_type.as_str(),
                                        permit.used_half_open_permit,
                                        false,
                                        Some(format!(
                                            "upstream returned {}",
                                            response.status().as_u16()
                                        )),
                                    )
                                    .await;
                            }

                            if claude_error_path && !provider_needs_transform {
                                last_error = Some(ForwardFailure::new(
                                    Some(provider.clone()),
                                    streaming_response_to_upstream_error(response),
                                ));
                            } else {
                                pending_upstream_response =
                                    Some(ForwardedResponse { provider, response });
                                last_error = Some(ForwardFailure::new(
                                    pending_upstream_response
                                        .as_ref()
                                        .map(|response| response.provider.clone()),
                                    ProxyError::UpstreamError {
                                        status: pending_upstream_response
                                            .as_ref()
                                            .expect("pending upstream response")
                                            .response
                                            .status()
                                            .as_u16(),
                                        body: None,
                                    },
                                ));
                            }
                            continue;
                        }
                        _ => {
                            if !bypass_circuit_breaker {
                                let _ = self
                                    .router
                                    .record_result(
                                        &provider.id,
                                        app_type.as_str(),
                                        permit.used_half_open_permit,
                                        false,
                                        Some(format!(
                                            "upstream returned {}",
                                            response.status().as_u16()
                                        )),
                                    )
                                    .await;
                            }

                            return Ok(ForwardedResponse { provider, response });
                        }
                    }
                }
                Err(StreamingRequestError::BeforeResponse(error))
                | Err(StreamingRequestError::AfterResponse(error)) => {
                    match classify_attempt_error(&error, app_type, &provider) {
                        AttemptDecision::ProviderFailure => {
                            if !bypass_circuit_breaker {
                                let _ = self
                                    .router
                                    .record_result(
                                        &provider.id,
                                        app_type.as_str(),
                                        permit.used_half_open_permit,
                                        false,
                                        Some(error.to_string()),
                                    )
                                    .await;
                            }
                            last_error = Some(ForwardFailure::new(Some(provider.clone()), error));
                        }
                        AttemptDecision::NeutralRelease | AttemptDecision::FatalStop => {
                            if !bypass_circuit_breaker {
                                self.router
                                    .release_permit_neutral(
                                        &provider.id,
                                        app_type.as_str(),
                                        permit.used_half_open_permit,
                                    )
                                    .await;
                            }
                            return Err(ForwardFailure::new(Some(provider), error));
                        }
                    }
                }
            }
        }

        if let Some(response) = pending_upstream_response {
            return Ok(response);
        }

        if attempted_provider {
            Err(last_error
                .unwrap_or_else(|| ForwardFailure::new(None, ProxyError::NoAvailableProvider)))
        } else {
            Err(ForwardFailure::new(None, ProxyError::NoAvailableProvider))
        }
    }

    #[allow(dead_code)]
    #[expect(
        clippy::too_many_arguments,
        reason = "forwarding requires request, provider, and retry options"
    )]
    pub async fn forward_buffered_response(
        &self,
        app_type: &AppType,
        endpoint: &str,
        body: Value,
        headers: &HeaderMap,
        providers: Vec<Provider>,
        options: ForwardOptions,
        rectifier_config: RectifierConfig,
    ) -> Result<ForwardedResponse<BufferedResponse>, ProxyError> {
        self.forward_buffered_response_detailed(
            app_type,
            endpoint,
            body,
            headers,
            providers,
            options,
            rectifier_config,
        )
        .await
        .map_err(|failure| failure.error)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "forwarding requires request, provider, and retry options"
    )]
    pub async fn forward_buffered_response_detailed(
        &self,
        app_type: &AppType,
        endpoint: &str,
        body: Value,
        headers: &HeaderMap,
        providers: Vec<Provider>,
        options: ForwardOptions,
        rectifier_config: RectifierConfig,
    ) -> Result<ForwardedResponse<BufferedResponse>, ForwardFailure> {
        if providers.is_empty() {
            return Err(ForwardFailure::new(None, ProxyError::NoAvailableProvider));
        }

        let claude_error_path = matches!(app_type, AppType::Claude);
        let bypass_circuit_breaker = options.bypass_circuit_breaker;
        let mut last_error = None;
        let mut attempted_provider = false;
        let mut attempted_providers = 0usize;
        let mut pending_upstream_response = None;
        let max_attempts = (options.max_retries as usize).saturating_add(1);

        for provider in providers {
            if attempted_providers >= max_attempts {
                break;
            }

            let permit = if bypass_circuit_breaker {
                super::circuit_breaker::AllowResult {
                    allowed: true,
                    used_half_open_permit: false,
                }
            } else {
                self.router
                    .allow_provider_request(&provider.id, app_type.as_str())
                    .await
            };

            if !permit.allowed {
                continue;
            }

            attempted_provider = true;
            attempted_providers += 1;
            pending_upstream_response = None;
            let provider_needs_transform = matches!(app_type, AppType::Claude)
                && get_adapter(app_type).needs_transform(&provider);

            match self
                .send_buffered_request(
                    app_type,
                    &provider,
                    endpoint,
                    &body,
                    headers,
                    ForwardOptions {
                        max_retries: 0,
                        ..options
                    },
                    &rectifier_config,
                )
                .await
            {
                Ok(outcome) => {
                    let response = outcome.response;
                    if response.status.is_success() {
                        if !bypass_circuit_breaker {
                            let _ = self
                                .router
                                .record_result(
                                    &provider.id,
                                    app_type.as_str(),
                                    permit.used_half_open_permit,
                                    true,
                                    None,
                                )
                                .await;
                        }

                        return Ok(ForwardedResponse { provider, response });
                    }

                    match outcome.attempt_decision {
                        AttemptDecision::NeutralRelease => {
                            if !bypass_circuit_breaker {
                                self.router
                                    .release_permit_neutral(
                                        &provider.id,
                                        app_type.as_str(),
                                        permit.used_half_open_permit,
                                    )
                                    .await;
                            }

                            if claude_error_path && !provider_needs_transform {
                                return Err(ForwardFailure::new(
                                    Some(provider),
                                    buffered_response_to_upstream_error(response),
                                ));
                            }

                            return Ok(ForwardedResponse { provider, response });
                        }
                        AttemptDecision::ProviderFailure => {
                            if !bypass_circuit_breaker {
                                let _ = self
                                    .router
                                    .record_result(
                                        &provider.id,
                                        app_type.as_str(),
                                        permit.used_half_open_permit,
                                        false,
                                        Some(format!(
                                            "upstream returned {}",
                                            response.status.as_u16()
                                        )),
                                    )
                                    .await;
                            }

                            if claude_error_path && !provider_needs_transform {
                                last_error = Some(ForwardFailure::new(
                                    Some(provider.clone()),
                                    buffered_response_to_upstream_error(response),
                                ));
                            } else {
                                pending_upstream_response =
                                    Some(ForwardedResponse { provider, response });
                                last_error = Some(ForwardFailure::new(
                                    pending_upstream_response
                                        .as_ref()
                                        .map(|response| response.provider.clone()),
                                    ProxyError::UpstreamError {
                                        status: pending_upstream_response
                                            .as_ref()
                                            .expect("pending upstream response")
                                            .response
                                            .status
                                            .as_u16(),
                                        body: None,
                                    },
                                ));
                            }
                            continue;
                        }
                        _ => {
                            if !bypass_circuit_breaker {
                                let _ = self
                                    .router
                                    .record_result(
                                        &provider.id,
                                        app_type.as_str(),
                                        permit.used_half_open_permit,
                                        false,
                                        Some(format!(
                                            "upstream returned {}",
                                            response.status.as_u16()
                                        )),
                                    )
                                    .await;
                            }

                            return Ok(ForwardedResponse { provider, response });
                        }
                    }
                }
                Err(BufferedRequestError::BeforeResponse(error))
                | Err(BufferedRequestError::AfterResponse(error)) => {
                    match classify_attempt_error(&error, app_type, &provider) {
                        AttemptDecision::ProviderFailure => {
                            if !bypass_circuit_breaker {
                                let _ = self
                                    .router
                                    .record_result(
                                        &provider.id,
                                        app_type.as_str(),
                                        permit.used_half_open_permit,
                                        false,
                                        Some(error.to_string()),
                                    )
                                    .await;
                            }
                            last_error = Some(ForwardFailure::new(Some(provider.clone()), error));
                        }
                        AttemptDecision::NeutralRelease | AttemptDecision::FatalStop => {
                            if !bypass_circuit_breaker {
                                self.router
                                    .release_permit_neutral(
                                        &provider.id,
                                        app_type.as_str(),
                                        permit.used_half_open_permit,
                                    )
                                    .await;
                            }
                            return Err(ForwardFailure::new(Some(provider), error));
                        }
                    }
                }
            }
        }

        if let Some(response) = pending_upstream_response {
            return Ok(response);
        }

        if attempted_provider {
            Err(last_error
                .unwrap_or_else(|| ForwardFailure::new(None, ProxyError::NoAvailableProvider)))
        } else {
            Err(ForwardFailure::new(None, ProxyError::NoAvailableProvider))
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "request execution needs provider, endpoint, headers, and retry options"
    )]
    async fn send_streaming_request(
        &self,
        app_type: &AppType,
        provider: &Provider,
        endpoint: &str,
        body: &Value,
        headers: &HeaderMap,
        options: ForwardOptions,
        rectifier_config: &RectifierConfig,
    ) -> Result<StreamingAttemptOutcome, StreamingRequestError> {
        let started_at = Instant::now();
        let allow_transport_retry = uses_internal_transport_retry(app_type);
        let mut request_body = body.clone();
        let mut rectifier_retried = false;

        'request_loop: loop {
            let base_request = self
                .prepare_request(
                    app_type,
                    provider,
                    endpoint,
                    &request_body,
                    headers,
                    options,
                )
                .await
                .map_err(StreamingRequestError::BeforeResponse)?;
            let mut attempt = 0u32;

            loop {
                let attempt_started_at = if allow_transport_retry {
                    Instant::now()
                } else {
                    started_at
                };
                let remaining_timeout = match options.request_timeout {
                    Some(request_timeout) => {
                        let remaining_timeout =
                            request_timeout.saturating_sub(attempt_started_at.elapsed());
                        if remaining_timeout.is_zero() {
                            let timeout_error = request_timeout_error(request_timeout);
                            return Err(if rectifier_retried {
                                StreamingRequestError::AfterResponse(timeout_error)
                            } else {
                                StreamingRequestError::BeforeResponse(timeout_error)
                            });
                        }
                        Some(remaining_timeout)
                    }
                    None => None,
                };

                let request =
                    clone_request(&base_request).map_err(StreamingRequestError::BeforeResponse)?;

                match match remaining_timeout {
                    Some(remaining_timeout) => {
                        tokio::time::timeout(remaining_timeout, request.send())
                            .await
                            .map_err(|_| ())
                    }
                    None => Ok(request.send().await),
                } {
                    Ok(Ok(response)) => {
                        if response.status().is_success() {
                            let response = prepare_success_streaming_response(
                                response,
                                attempt_started_at,
                                options.request_timeout,
                                uses_responses_protocol(app_type, provider, endpoint),
                            )
                            .await
                            .map_err(|error| {
                                if rectifier_retried {
                                    StreamingRequestError::AfterResponse(error)
                                } else {
                                    StreamingRequestError::BeforeResponse(error)
                                }
                            })?;
                            return Ok(StreamingAttemptOutcome {
                                response: StreamingResponse::Live(response),
                                attempt_decision: AttemptDecision::FatalStop,
                            });
                        }

                        if should_buffer_streaming_error_response(app_type, response.status()) {
                            let buffered_response = read_streaming_error_response(
                                response,
                                attempt_started_at,
                                options.request_timeout,
                            )
                            .await
                            .map_err(StreamingRequestError::AfterResponse)?;

                            if !rectifier_retried {
                                if let Some(rectified_body) = maybe_rectify_claude_buffered_request(
                                    app_type,
                                    &buffered_response,
                                    &request_body,
                                    rectifier_config,
                                ) {
                                    rectifier_retried = true;
                                    request_body = rectified_body;
                                    continue 'request_loop;
                                }
                            }

                            return Ok(StreamingAttemptOutcome {
                                attempt_decision: classify_upstream_response(
                                    buffered_response.status,
                                    rectifier_retried,
                                    app_type,
                                    provider,
                                ),
                                response: StreamingResponse::Buffered(buffered_response),
                            });
                        }

                        return Ok(StreamingAttemptOutcome {
                            attempt_decision: classify_upstream_response(
                                response.status(),
                                rectifier_retried,
                                app_type,
                                provider,
                            ),
                            response: StreamingResponse::Live(LiveResponse::from_reqwest(response)),
                        });
                    }
                    Ok(Err(error)) => {
                        if allow_transport_retry
                            && attempt < options.max_retries
                            && is_retryable_transport_error(&error)
                        {
                            attempt += 1;
                            continue;
                        }

                        let mapped_error = map_request_send_error(error, options.request_timeout);
                        return Err(if rectifier_retried {
                            StreamingRequestError::AfterResponse(mapped_error)
                        } else {
                            StreamingRequestError::BeforeResponse(mapped_error)
                        });
                    }
                    Err(_) => {
                        if allow_transport_retry && attempt < options.max_retries {
                            attempt += 1;
                            continue;
                        }

                        let timeout_error = request_timeout_error(
                            options
                                .request_timeout
                                .expect("request timeout should exist when timeout future errors"),
                        );
                        return Err(if rectifier_retried {
                            StreamingRequestError::AfterResponse(timeout_error)
                        } else {
                            StreamingRequestError::BeforeResponse(timeout_error)
                        });
                    }
                }
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "request execution needs provider, endpoint, headers, and retry options"
    )]
    async fn send_buffered_request(
        &self,
        app_type: &AppType,
        provider: &Provider,
        endpoint: &str,
        body: &Value,
        headers: &HeaderMap,
        options: ForwardOptions,
        rectifier_config: &RectifierConfig,
    ) -> Result<BufferedAttemptOutcome, BufferedRequestError> {
        let mut request_body = body.clone();
        let mut rectifier_retried = false;
        let request_started_at = Instant::now();
        let allow_transport_retry = uses_internal_transport_retry(app_type);

        'request_loop: loop {
            let base_request = self
                .prepare_request(
                    app_type,
                    provider,
                    endpoint,
                    &request_body,
                    headers,
                    options,
                )
                .await
                .map_err(BufferedRequestError::BeforeResponse)?;
            let mut attempt = 0u32;

            loop {
                let attempt_started_at = if allow_transport_retry {
                    Instant::now()
                } else {
                    request_started_at
                };
                let remaining_timeout = match options.request_timeout {
                    Some(request_timeout) => {
                        let remaining_timeout =
                            request_timeout.saturating_sub(attempt_started_at.elapsed());
                        if remaining_timeout.is_zero() {
                            let timeout_error = request_timeout_error(request_timeout);
                            return Err(if rectifier_retried {
                                BufferedRequestError::AfterResponse(timeout_error)
                            } else {
                                BufferedRequestError::BeforeResponse(timeout_error)
                            });
                        }
                        Some(remaining_timeout)
                    }
                    None => None,
                };

                let request =
                    clone_request(&base_request).map_err(BufferedRequestError::BeforeResponse)?;

                match match remaining_timeout {
                    Some(remaining_timeout) => {
                        tokio::time::timeout(remaining_timeout, request.send())
                            .await
                            .map_err(|_| ())
                    }
                    None => Ok(request.send().await),
                } {
                    Ok(Ok(response)) => {
                        let status = response.status();
                        let mut response_headers = response.headers().clone();
                        let response_body = match options.request_timeout {
                            Some(request_timeout) => {
                                let remaining_timeout =
                                    request_timeout.saturating_sub(attempt_started_at.elapsed());
                                if remaining_timeout.is_zero() {
                                    return Err(BufferedRequestError::AfterResponse(
                                        request_timeout_error(request_timeout),
                                    ));
                                }
                                tokio::time::timeout(remaining_timeout, response.bytes())
                                    .await
                                    .map_err(|_| {
                                        BufferedRequestError::AfterResponse(request_timeout_error(
                                            request_timeout,
                                        ))
                                    })?
                                    .map_err(|error| {
                                        BufferedRequestError::AfterResponse(map_request_send_error(
                                            error,
                                            Some(request_timeout),
                                        ))
                                    })?
                            }
                            None => response.bytes().await.map_err(|error| {
                                BufferedRequestError::AfterResponse(map_request_send_error(
                                    error, None,
                                ))
                            })?,
                        };
                        let response_body =
                            decode_buffered_response_body(&mut response_headers, response_body);

                        let buffered_response = BufferedResponse {
                            status,
                            headers: response_headers,
                            body: response_body,
                        };

                        if buffered_response.status.is_success()
                            && uses_responses_protocol(app_type, provider, endpoint)
                        {
                            validate_responses_success_body(&buffered_response.body).map_err(
                                |error| {
                                    if rectifier_retried {
                                        BufferedRequestError::AfterResponse(error)
                                    } else {
                                        BufferedRequestError::BeforeResponse(error)
                                    }
                                },
                            )?;
                        }

                        if !rectifier_retried {
                            if let Some(rectified_body) = maybe_rectify_claude_buffered_request(
                                app_type,
                                &buffered_response,
                                &request_body,
                                rectifier_config,
                            ) {
                                rectifier_retried = true;
                                request_body = rectified_body;
                                continue 'request_loop;
                            }
                        }

                        return Ok(BufferedAttemptOutcome {
                            attempt_decision: classify_upstream_response(
                                buffered_response.status,
                                rectifier_retried,
                                app_type,
                                provider,
                            ),
                            response: buffered_response,
                        });
                    }
                    Ok(Err(error)) => {
                        if allow_transport_retry
                            && attempt < options.max_retries
                            && is_retryable_transport_error(&error)
                        {
                            attempt += 1;
                            continue;
                        }

                        let mapped_error = map_request_send_error(error, options.request_timeout);
                        return Err(if rectifier_retried {
                            BufferedRequestError::AfterResponse(mapped_error)
                        } else {
                            BufferedRequestError::BeforeResponse(mapped_error)
                        });
                    }
                    Err(_) => {
                        if allow_transport_retry && attempt < options.max_retries {
                            attempt += 1;
                            continue;
                        }

                        let timeout_error = request_timeout_error(
                            options
                                .request_timeout
                                .expect("request timeout should exist when timeout future errors"),
                        );
                        return Err(if rectifier_retried {
                            BufferedRequestError::AfterResponse(timeout_error)
                        } else {
                            BufferedRequestError::BeforeResponse(timeout_error)
                        });
                    }
                }
            }
        }
    }
}

fn classify_attempt_error(
    error: &ProxyError,
    app_type: &AppType,
    provider: &Provider,
) -> AttemptDecision {
    if matches!(app_type, AppType::Codex)
        && provider.is_codex_official()
        && (matches!(error, ProxyError::AuthError(_))
            || matches!(
                error,
                ProxyError::UpstreamError {
                    status: 401 | 403,
                    ..
                }
            ))
    {
        return AttemptDecision::NeutralRelease;
    }

    match error {
        ProxyError::UpstreamError {
            status: 400 | 405 | 406 | 413 | 414 | 415 | 422 | 501,
            ..
        } => AttemptDecision::NeutralRelease,
        ProxyError::AlreadyRunning
        | ProxyError::NotRunning
        | ProxyError::BindFailed(_)
        | ProxyError::StopTimeout
        | ProxyError::StopFailed(_)
        | ProxyError::NoAvailableProvider
        | ProxyError::AllProvidersCircuitOpen
        | ProxyError::NoProvidersConfigured
        | ProxyError::DatabaseError(_)
        | ProxyError::InvalidRequest(_)
        | ProxyError::Internal(_) => AttemptDecision::FatalStop,
        _ => AttemptDecision::ProviderFailure,
    }
}

fn maybe_rectify_claude_buffered_request(
    app_type: &AppType,
    response: &BufferedResponse,
    request_body: &Value,
    rectifier_config: &RectifierConfig,
) -> Option<Value> {
    if *app_type != AppType::Claude {
        return None;
    }

    if !matches!(response.status.as_u16(), 400 | 422) {
        return None;
    }

    let error_message = extract_upstream_error_message(&response.body);

    if should_rectify_thinking_signature(error_message.as_deref(), rectifier_config) {
        let mut rectified_body = request_body.clone();
        let result = rectify_anthropic_request(&mut rectified_body);
        if result.applied {
            return Some(normalize_thinking_type(rectified_body));
        }
    }

    if should_rectify_thinking_budget(error_message.as_deref(), rectifier_config) {
        let mut rectified_body = request_body.clone();
        let result = rectify_thinking_budget(&mut rectified_body);
        if result.applied {
            return Some(normalize_thinking_type(rectified_body));
        }
    }

    None
}

fn should_buffer_streaming_error_response(app_type: &AppType, status: reqwest::StatusCode) -> bool {
    *app_type == AppType::Claude && !status.is_success()
}

fn uses_responses_protocol(app_type: &AppType, provider: &Provider, endpoint: &str) -> bool {
    if matches!(app_type, AppType::Claude) {
        return super::providers::get_claude_api_format(provider) == "openai_responses";
    }

    let path = endpoint.split_once('?').map_or(endpoint, |(path, _)| path);
    matches!(
        path,
        "/responses" | "/v1/responses" | "/responses/compact" | "/v1/responses/compact"
    ) && !super::providers::should_convert_codex_responses_to_chat(provider, endpoint)
}

async fn prepare_success_streaming_response(
    response: reqwest::Response,
    started_at: Instant,
    request_timeout: Option<Duration>,
    validate_responses_semantics: bool,
) -> Result<LiveResponse, ProxyError> {
    if validate_responses_semantics {
        return validate_responses_stream_start(response, started_at, request_timeout).await;
    }

    let Some(request_timeout) = request_timeout else {
        return Ok(LiveResponse::from_reqwest(response));
    };

    let status = response.status();
    let headers = response.headers().clone();
    let mut stream = response.bytes_stream().boxed();
    let remaining_timeout = request_timeout.saturating_sub(started_at.elapsed());
    if remaining_timeout.is_zero() {
        return Err(stream_first_byte_timeout_error(request_timeout));
    }

    let first = tokio::time::timeout(remaining_timeout, stream.next())
        .await
        .map_err(|_| stream_first_byte_timeout_error(request_timeout))?;
    let Some(first) = first else {
        return Err(ProxyError::ForwardFailed(
            "stream ended before the first response chunk".to_string(),
        ));
    };
    let first = first.map_err(|error| {
        ProxyError::ForwardFailed(format!("read first response chunk failed: {error}"))
    })?;

    let replay = futures::stream::once(async move { Ok(first) }).chain(stream);
    Ok(LiveResponse::from_stream(status, headers, replay))
}

async fn validate_responses_stream_start(
    response: reqwest::Response,
    started_at: Instant,
    request_timeout: Option<Duration>,
) -> Result<LiveResponse, ProxyError> {
    const MAX_PRIME_BYTES: usize = 256 * 1024;

    let status = response.status();
    let headers = response.headers().clone();
    let mut stream = response.bytes_stream().boxed();
    let mut replay_chunks = Vec::new();
    let mut replay_bytes = 0usize;
    let mut parse_buffer = String::new();
    let mut utf8_remainder = Vec::new();

    loop {
        let next = match request_timeout {
            Some(request_timeout) => {
                let remaining_timeout = request_timeout.saturating_sub(started_at.elapsed());
                if remaining_timeout.is_zero() {
                    return Err(stream_first_byte_timeout_error(request_timeout));
                }
                tokio::time::timeout(remaining_timeout, stream.next())
                    .await
                    .map_err(|_| stream_first_byte_timeout_error(request_timeout))?
            }
            None => stream.next().await,
        };

        let Some(chunk) = next else {
            if let Some(outcome) = inspect_responses_json_document(&parse_buffer) {
                outcome?;
                return Ok(LiveResponse::from_stream(
                    status,
                    headers,
                    futures::stream::iter(replay_chunks.into_iter().map(Ok)),
                ));
            }
            if !parse_buffer.trim().is_empty() {
                if let Some(outcome) = inspect_responses_start_event(parse_buffer.trim()) {
                    outcome?;
                    return Ok(LiveResponse::from_stream(
                        status,
                        headers,
                        futures::stream::iter(replay_chunks.into_iter().map(Ok)),
                    ));
                }
            }
            return Err(ProxyError::ForwardFailed(
                "Responses stream ended before producing output or a terminal event".to_string(),
            ));
        };
        let chunk = chunk.map_err(|error| {
            ProxyError::ForwardFailed(format!(
                "failed while validating Responses stream start: {error}"
            ))
        })?;
        super::sse::append_utf8_safe(&mut parse_buffer, &mut utf8_remainder, &chunk);
        replay_bytes = replay_bytes.saturating_add(chunk.len());
        replay_chunks.push(chunk);

        if let Some(outcome) = inspect_responses_json_document(&parse_buffer) {
            outcome?;
            let replay = futures::stream::iter(replay_chunks.into_iter().map(Ok)).chain(stream);
            return Ok(LiveResponse::from_stream(status, headers, replay));
        }

        while let Some(block) = super::sse::take_sse_block(&mut parse_buffer) {
            if let Some(outcome) = inspect_responses_start_event(&block) {
                outcome?;
                let replay = futures::stream::iter(replay_chunks.into_iter().map(Ok)).chain(stream);
                return Ok(LiveResponse::from_stream(status, headers, replay));
            }
        }

        if replay_bytes >= MAX_PRIME_BYTES {
            log::warn!(
                "Responses semantic stream priming exceeded {MAX_PRIME_BYTES} bytes; committing stream"
            );
            let replay = futures::stream::iter(replay_chunks.into_iter().map(Ok)).chain(stream);
            return Ok(LiveResponse::from_stream(status, headers, replay));
        }
    }
}

fn validate_responses_success_body(body: &[u8]) -> Result<(), ProxyError> {
    if let Some(message) = responses_error_envelope_message(body) {
        return Err(ProxyError::TransformError(format!(
            "Responses upstream returned a 2xx failure: {message}"
        )));
    }
    Ok(())
}

fn responses_error_envelope_message(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let status = value.get("status").and_then(Value::as_str);
    let has_error = value.get("error").is_some_and(|error| !error.is_null());
    if !matches!(status, Some("failed" | "cancelled")) && !has_error {
        return None;
    }

    let error = value.get("error").unwrap_or(&value);
    let error_type = error
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| error.get("code").and_then(Value::as_str))
        .unwrap_or_else(|| status.unwrap_or("error"));
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .filter(|message| !message.trim().is_empty())
        .unwrap_or(match status {
            Some("cancelled") => "response generation was cancelled",
            _ => "response generation failed",
        });
    Some(format!("{error_type}: {message}"))
}

fn inspect_responses_json_document(buffer: &str) -> Option<Result<(), ProxyError>> {
    let trimmed = buffer.trim();
    if !matches!(trimmed.as_bytes().first(), Some(b'{') | Some(b'[')) {
        return None;
    }
    let _: Value = serde_json::from_str(trimmed).ok()?;
    Some(validate_responses_success_body(trimmed.as_bytes()))
}

fn inspect_responses_start_event(block: &str) -> Option<Result<(), ProxyError>> {
    let mut named_event = None;
    let mut data_lines = Vec::new();
    for line in block.lines() {
        if let Some(event) = super::sse::strip_sse_field(line, "event") {
            named_event = Some(event.trim().to_string());
        } else if let Some(data) = super::sse::strip_sse_field(line, "data") {
            data_lines.push(data);
        }
    }
    if data_lines.is_empty() {
        return None;
    }
    let value: Value = match serde_json::from_str(&data_lines.join("\n")) {
        Ok(value) => value,
        Err(_) => return None,
    };
    let event = named_event
        .as_deref()
        .filter(|event| !event.is_empty())
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or("");

    let response = value.get("response").unwrap_or(&value);
    if matches!(
        response.get("status").and_then(Value::as_str),
        Some("failed" | "cancelled")
    ) || response.get("error").is_some_and(|error| !error.is_null())
    {
        let error = response.get("error").unwrap_or(response);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| error.as_str())
            .unwrap_or("Responses upstream failed before output");
        let error_type = error
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| error.get("code").and_then(Value::as_str))
            .or_else(|| response.get("status").and_then(Value::as_str))
            .unwrap_or("upstream_error");
        return Some(Err(ProxyError::TransformError(format!(
            "Responses upstream {error_type}: {message}"
        ))));
    }

    match event {
        "response.failed" | "error" => {
            let error = response.get("error").unwrap_or(response);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .unwrap_or("Responses upstream emitted an error before output");
            let error_type = error
                .get("type")
                .and_then(Value::as_str)
                .or_else(|| error.get("code").and_then(Value::as_str))
                .unwrap_or("upstream_error");
            Some(Err(ProxyError::TransformError(format!(
                "Responses upstream {error_type}: {message}"
            ))))
        }
        "response.created" | "response.in_progress" | "response.queued" | "" => None,
        _ => Some(Ok(())),
    }
}

async fn read_streaming_error_response(
    response: reqwest::Response,
    started_at: Instant,
    request_timeout: Option<Duration>,
) -> Result<BufferedResponse, ProxyError> {
    let status = response.status();
    let mut headers = response.headers().clone();
    let body = match request_timeout {
        Some(request_timeout) => {
            let remaining_timeout = request_timeout.saturating_sub(started_at.elapsed());
            if remaining_timeout.is_zero() {
                return Err(stream_first_byte_timeout_error(request_timeout));
            }

            tokio::time::timeout(remaining_timeout, response.bytes())
                .await
                .map_err(|_| stream_first_byte_timeout_error(request_timeout))?
                .map_err(|error| map_request_send_error(error, Some(request_timeout)))?
        }
        None => response
            .bytes()
            .await
            .map_err(|error| map_request_send_error(error, None))?,
    };
    let body = decode_buffered_response_body(&mut headers, body);

    Ok(BufferedResponse {
        status,
        headers,
        body,
    })
}

fn extract_upstream_error_message(body: &[u8]) -> Option<String> {
    if let Ok(json_body) = serde_json::from_slice::<Value>(body) {
        return [
            json_body.pointer("/error/message"),
            json_body.pointer("/message"),
            json_body.pointer("/detail"),
            json_body.pointer("/error"),
        ]
        .into_iter()
        .flatten()
        .find_map(|value| value.as_str().map(ToString::to_string));
    }

    std::str::from_utf8(body)
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn upstream_error_body_from_bytes(body: &[u8]) -> Option<String> {
    if body.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(body).into_owned())
    }
}

fn buffered_response_to_upstream_error(response: BufferedResponse) -> ProxyError {
    ProxyError::UpstreamError {
        status: response.status.as_u16(),
        body: upstream_error_body_from_bytes(&response.body),
    }
}

fn streaming_response_to_upstream_error(response: StreamingResponse) -> ProxyError {
    match response {
        StreamingResponse::Buffered(response) => buffered_response_to_upstream_error(response),
        StreamingResponse::Live(response) => ProxyError::UpstreamError {
            status: response.status().as_u16(),
            body: None,
        },
    }
}

fn clone_request(
    base_request: &reqwest::RequestBuilder,
) -> Result<reqwest::RequestBuilder, ProxyError> {
    base_request.try_clone().ok_or_else(|| {
        ProxyError::ForwardFailed("clone proxy request failed before retry".to_string())
    })
}

fn uses_internal_transport_retry(app_type: &AppType) -> bool {
    !matches!(app_type, AppType::Claude)
}

fn is_retryable_transport_error(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

fn map_request_send_error(error: reqwest::Error, request_timeout: Option<Duration>) -> ProxyError {
    if error.is_timeout() {
        return match request_timeout {
            Some(request_timeout) => request_timeout_error(request_timeout),
            None => ProxyError::Timeout(error.to_string()),
        };
    }

    if error.is_connect() {
        return ProxyError::ForwardFailed(format!("connection failed: {error}"));
    }

    ProxyError::ForwardFailed(error.to_string())
}

fn request_timeout_error(request_timeout: Duration) -> ProxyError {
    ProxyError::Timeout(format!(
        "request timed out after {}s",
        request_timeout.as_secs()
    ))
}

fn stream_first_byte_timeout_error(request_timeout: Duration) -> ProxyError {
    let display_seconds = request_timeout
        .as_secs()
        .max(u64::from(!request_timeout.is_zero()));
    ProxyError::Timeout(format!("stream timeout after {}s", display_seconds))
}

fn classify_upstream_response(
    status: reqwest::StatusCode,
    rectifier_retried: bool,
    app_type: &AppType,
    provider: &Provider,
) -> AttemptDecision {
    if matches!(app_type, AppType::Codex)
        && provider.is_codex_official()
        && matches!(status.as_u16(), 401 | 403)
    {
        return AttemptDecision::NeutralRelease;
    }

    match status.as_u16() {
        400 | 422 if rectifier_retried => AttemptDecision::NeutralRelease,
        400 | 405 | 406 | 413 | 414 | 415 | 422 | 501 => AttemptDecision::NeutralRelease,
        _ => AttemptDecision::ProviderFailure,
    }
}

#[cfg(test)]
mod tests;
