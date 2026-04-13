use axum::http::HeaderMap;
use bytes::Bytes;
use serde_json::Value;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{app_config::AppType, provider::Provider};

use super::{
    error::ProxyError,
    provider_router::ProviderRouter,
    providers::get_adapter,
    thinking_budget_rectifier::{rectify_thinking_budget, should_rectify_thinking_budget},
    thinking_rectifier::{
        normalize_thinking_type, rectify_anthropic_request, should_rectify_thinking_signature,
    },
    types::OptimizerConfig,
    types::RectifierConfig,
};

mod request_builder;

pub struct RequestForwarder {
    router: Arc<ProviderRouter>,
    optimizer_config: OptimizerConfig,
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

#[derive(Debug)]
pub enum StreamingResponse {
    Live(reqwest::Response),
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
        })
    }

    pub fn with_optimizer_config(mut self, optimizer_config: OptimizerConfig) -> Self {
        self.optimizer_config = optimizer_config;
        self
    }

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
        let bypass_circuit_breaker = options.bypass_circuit_breaker || providers.len() == 1;
        let mut last_error = None;
        let mut attempted_provider = false;
        let mut pending_upstream_response = None;

        for provider in providers {
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
                    options,
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
                Err(StreamingRequestError::AfterResponse(error)) => {
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
                Err(StreamingRequestError::BeforeResponse(error)) => {
                    match classify_attempt_error(&error) {
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
        let bypass_circuit_breaker = options.bypass_circuit_breaker || providers.len() == 1;
        let mut last_error = None;
        let mut attempted_provider = false;
        let mut pending_upstream_response = None;

        for provider in providers {
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
                    options,
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
                Err(BufferedRequestError::AfterResponse(error)) => {
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
                Err(BufferedRequestError::BeforeResponse(error)) => {
                    match classify_attempt_error(&error) {
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
                            .map(|result| result)
                    }
                    None => Ok(request.send().await),
                } {
                    Ok(Ok(response)) => {
                        if response.status().is_success() {
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
                                ),
                                response: StreamingResponse::Buffered(buffered_response),
                            });
                        }

                        return Ok(StreamingAttemptOutcome {
                            attempt_decision: classify_upstream_response(
                                response.status(),
                                rectifier_retried,
                            ),
                            response: StreamingResponse::Live(response),
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
                            .map(|result| result)
                    }
                    None => Ok(request.send().await),
                } {
                    Ok(Ok(response)) => {
                        let status = response.status();
                        let response_headers = response.headers().clone();
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

                        let buffered_response = BufferedResponse {
                            status,
                            headers: response_headers,
                            body: response_body,
                        };

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

fn classify_attempt_error(error: &ProxyError) -> AttemptDecision {
    match error {
        ProxyError::UpstreamError {
            status: 400 | 422, ..
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

async fn read_streaming_error_response(
    response: reqwest::Response,
    started_at: Instant,
    request_timeout: Option<Duration>,
) -> Result<BufferedResponse, ProxyError> {
    let status = response.status();
    let headers = response.headers().clone();
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
) -> AttemptDecision {
    match status.as_u16() {
        400 | 422 if rectifier_retried => AttemptDecision::NeutralRelease,
        400 | 422 => AttemptDecision::ProviderFailure,
        _ => AttemptDecision::ProviderFailure,
    }
}

#[cfg(test)]
mod tests;
