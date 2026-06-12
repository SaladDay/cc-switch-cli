use axum::{
    body::Body,
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures::StreamExt;

use crate::{app_config::AppType, provider::Provider};

use super::{
    error::ProxyError,
    metrics::estimate_tokens_from_char_count,
    response::{PreparedResponse, StreamCompletion},
    server::ProxyServerState,
    usage::{
        log_buffered_response, log_error_request, log_stream_response, RequestLogContext,
        StreamLogCollector,
    },
};

#[cfg(test)]
mod tests;

pub struct ResponseHandler;

#[derive(Clone)]
pub struct SuccessSyncInfo {
    pub app_type: AppType,
    pub provider: Provider,
    pub current_provider_id_at_start: String,
    /// 当为 true 时，跳过 set_current_provider / update_live_backup，
    /// 因为 provider 是模型路由命中选中的，不是用户主动切换的。
    pub is_model_routed: bool,
}

impl ResponseHandler {
    pub async fn finish_buffered(
        state: &ProxyServerState,
        response_result: Result<PreparedResponse, ProxyError>,
        status: reqwest::StatusCode,
        success_sync: Option<SuccessSyncInfo>,
        request_log: Option<RequestLogContext>,
    ) -> Response {
        match response_result {
            Ok(response) => {
                let PreparedResponse {
                    response,
                    estimated_output_tokens,
                    upstream_error_summary,
                    body_bytes,
                    ..
                } = response;
                if let (Some(request_log), Some(body_bytes)) =
                    (request_log.as_ref(), body_bytes.as_ref())
                {
                    log_buffered_response(state, request_log, status.as_u16(), body_bytes).await;
                }
                state
                    .record_estimated_output_tokens(estimated_output_tokens)
                    .await;
                if let Some(ref sync) = success_sync {
                    state
                        .record_provider_activity(&sync.provider.id, estimated_output_tokens)
                        .await;
                }
                if status.is_success() {
                    if let Some(success_sync) = success_sync {
                        state
                            .sync_successful_provider_selection(
                                &success_sync.app_type,
                                &success_sync.provider,
                                &success_sync.current_provider_id_at_start,
                                success_sync.is_model_routed,
                            )
                            .await;
                    }
                    state.record_request_success().await;
                } else {
                    state
                        .record_upstream_failure(status, upstream_error_summary)
                        .await;
                }
                response
            }
            Err(error) => {
                if let Some(request_log) = request_log.as_ref() {
                    log_error_request(state, request_log, &error).await;
                }
                state.record_request_error(&error).await;
                proxy_error_response(error)
            }
        }
    }

    pub async fn finish_streaming(
        state: &ProxyServerState,
        response_result: Result<PreparedResponse, ProxyError>,
        status: reqwest::StatusCode,
        success_sync: Option<SuccessSyncInfo>,
        request_log: Option<RequestLogContext>,
    ) -> Response {
        match response_result {
            Ok(response) => {
                track_streaming_response(state.clone(), response, status, success_sync, request_log)
            }
            Err(error) => {
                if let Some(request_log) = request_log.as_ref() {
                    log_error_request(state, request_log, &error).await;
                }
                state.record_request_error(&error).await;
                proxy_error_response(error)
            }
        }
    }
}

fn track_streaming_response(
    state: ProxyServerState,
    response: PreparedResponse,
    status: reqwest::StatusCode,
    success_sync: Option<SuccessSyncInfo>,
    request_log: Option<RequestLogContext>,
) -> Response {
    let PreparedResponse {
        response,
        stream_completion,
        upstream_error_summary,
        body_bytes,
        ..
    } = response;
    let (parts, body) = response.into_parts();
    let mut recorder = StreamingOutcomeRecorder::new(
        state,
        stream_completion,
        status,
        upstream_error_summary,
        body_bytes,
        success_sync,
        request_log,
    );
    let tracked_stream = async_stream::stream! {
        let mut stream = body.into_data_stream();

        while let Some(next) = stream.next().await {
            match next {
                Ok(chunk) => {
                    recorder.record_chunk(&chunk);
                    yield Ok(chunk)
                }
                Err(error) => {
                    recorder.finish();
                    yield Err(std::io::Error::other(error));
                    return;
                }
            }
        }

        recorder.finish();
    };

    Response::from_parts(parts, Body::from_stream(tracked_stream))
}

struct StreamingOutcomeRecorder {
    state: ProxyServerState,
    stream_completion: Option<StreamCompletion>,
    status: reqwest::StatusCode,
    upstream_error_summary: Option<String>,
    body_bytes: Option<Bytes>,
    success_sync: Option<SuccessSyncInfo>,
    request_log: Option<RequestLogContext>,
    log_collector: Option<StreamLogCollector>,
    output_char_count: u64,
    finished: bool,
}

impl StreamingOutcomeRecorder {
    fn new(
        state: ProxyServerState,
        stream_completion: Option<StreamCompletion>,
        status: reqwest::StatusCode,
        upstream_error_summary: Option<String>,
        body_bytes: Option<Bytes>,
        success_sync: Option<SuccessSyncInfo>,
        request_log: Option<RequestLogContext>,
    ) -> Self {
        let log_collector = request_log
            .as_ref()
            .map(|request_log| StreamLogCollector::new(request_log.started_at));
        Self {
            state,
            stream_completion,
            status,
            upstream_error_summary,
            body_bytes,
            success_sync,
            request_log,
            log_collector,
            output_char_count: 0,
            finished: false,
        }
    }

    fn record_chunk(&mut self, chunk: &bytes::Bytes) {
        self.output_char_count = self
            .output_char_count
            .saturating_add(String::from_utf8_lossy(chunk).chars().count() as u64);
        if let Some(log_collector) = self.log_collector.as_mut() {
            log_collector.record_chunk(chunk);
        }
    }

    fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;

        let state = self.state.clone();
        let estimated_output_tokens = estimate_tokens_from_char_count(self.output_char_count);
        let request_log = self.request_log.clone();
        let log_collector = self.log_collector.clone();
        let body_bytes = self.body_bytes.clone();
        if !self.status.is_success() {
            let status = self.status;
            let upstream_error_summary = self.upstream_error_summary.clone();
            tokio::spawn(async move {
                if let Some(request_log) = request_log.as_ref() {
                    if let Some(body_bytes) = body_bytes.as_ref() {
                        log_buffered_response(&state, request_log, status.as_u16(), body_bytes)
                            .await;
                    } else if let Some(log_collector) = log_collector.as_ref() {
                        log_stream_response(&state, request_log, status.as_u16(), log_collector)
                            .await;
                    }
                }
                state
                    .record_estimated_output_tokens(estimated_output_tokens)
                    .await;
                state
                    .record_upstream_failure(status, upstream_error_summary)
                    .await;
            });
            return;
        }

        match self
            .stream_completion
            .as_ref()
            .and_then(StreamCompletion::outcome)
        {
            Some(Err(message)) => {
                tokio::spawn(async move {
                    if let Some(request_log) = request_log.as_ref() {
                        log_error_request(
                            &state,
                            request_log,
                            &ProxyError::RequestFailed(message.clone()),
                        )
                        .await;
                    }
                    state
                        .record_estimated_output_tokens(estimated_output_tokens)
                        .await;
                    state.record_request_error_message(message).await;
                });
            }
            Some(Ok(())) => {
                let success_sync = self.success_sync.clone();
                tokio::spawn(async move {
                    if let Some(request_log) = request_log.as_ref() {
                        if let Some(body_bytes) = body_bytes.as_ref() {
                            log_buffered_response(
                                &state,
                                request_log,
                                reqwest::StatusCode::OK.as_u16(),
                                body_bytes,
                            )
                            .await;
                        } else if let Some(log_collector) = log_collector.as_ref() {
                            log_stream_response(
                                &state,
                                request_log,
                                reqwest::StatusCode::OK.as_u16(),
                                log_collector,
                            )
                            .await;
                        }
                    }
                    state
                        .record_estimated_output_tokens(estimated_output_tokens)
                        .await;
                    if let Some(ref sync) = success_sync {
                        state
                            .record_provider_activity(&sync.provider.id, estimated_output_tokens)
                            .await;
                        state
                            .sync_successful_provider_selection(
                                &sync.app_type,
                                &sync.provider,
                                &sync.current_provider_id_at_start,
                                sync.is_model_routed,
                            )
                            .await;
                    }
                    state.record_request_success().await;
                });
            }
            None if body_bytes.is_some() => {
                let success_sync = self.success_sync.clone();
                let status = self.status;
                tokio::spawn(async move {
                    if let Some(request_log) = request_log.as_ref() {
                        if let Some(body_bytes) = body_bytes.as_ref() {
                            log_buffered_response(&state, request_log, status.as_u16(), body_bytes)
                                .await;
                        }
                    }
                    state
                        .record_estimated_output_tokens(estimated_output_tokens)
                        .await;
                    if let Some(ref sync) = success_sync {
                        state
                            .record_provider_activity(&sync.provider.id, estimated_output_tokens)
                            .await;
                        state
                            .sync_successful_provider_selection(
                                &sync.app_type,
                                &sync.provider,
                                &sync.current_provider_id_at_start,
                                sync.is_model_routed,
                            )
                            .await;
                    }
                    state.record_request_success().await;
                });
            }
            None => {
                tokio::spawn(async move {
                    if let Some(request_log) = request_log.as_ref() {
                        log_error_request(
                            &state,
                            request_log,
                            &ProxyError::RequestFailed(
                                "stream terminated before completion".to_string(),
                            ),
                        )
                        .await;
                    }
                    state
                        .record_estimated_output_tokens(estimated_output_tokens)
                        .await;
                    state
                        .record_request_error_message(
                            "stream terminated before completion".to_string(),
                        )
                        .await;
                });
            }
        }
    }
}

impl Drop for StreamingOutcomeRecorder {
    fn drop(&mut self) {
        self.finish();
    }
}

pub fn proxy_error_response(error: ProxyError) -> Response {
    error.into_response()
}
