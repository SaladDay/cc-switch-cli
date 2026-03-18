use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    app_config::AppType,
    provider::Provider,
    proxy::{error::ProxyError, handler_context::HandlerContext, server::ProxyServerState},
};

use super::{
    calculator::{
        calculate_cost, format_decimal, lookup_model_pricing, pricing_model, resolve_pricing_config,
    },
    parser::{
        error_message_from_response_bytes, fallback_model_from_response_bytes,
        parse_claude_response_usage, ParsedUsage, StreamLogCollector, TokenUsage,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageLogPolicy {
    Passthrough,
    Transformed,
}

impl UsageLogPolicy {
    fn logs_zero_usage_on_parse_failure(self, status_code: u16) -> bool {
        let _ = self;
        let _ = status_code;
        true
    }
}

#[derive(Clone)]
pub struct RequestLogContext {
    pub app_type: AppType,
    pub provider: Provider,
    pub request_model: String,
    pub session_id: String,
    pub started_at: std::time::Instant,
    pub is_streaming: bool,
    pub policy: UsageLogPolicy,
}

impl RequestLogContext {
    pub fn from_handler(
        context: &HandlerContext,
        provider: Provider,
        is_streaming: bool,
        policy: UsageLogPolicy,
    ) -> Self {
        Self {
            app_type: context.app_type.clone(),
            provider,
            request_model: context.request_model.clone(),
            session_id: context.session_id.clone(),
            started_at: context.start_time,
            is_streaming,
            policy,
        }
    }

    fn latency_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }
}

pub async fn log_buffered_response(
    state: &ProxyServerState,
    context: &RequestLogContext,
    status_code: u16,
    body: &[u8],
) {
    if !logging_enabled(state).await {
        return;
    }

    if let Some(parsed) = parse_claude_response_usage(body) {
        let model = non_empty_model(&parsed, &context.request_model);
        insert_request_log(
            state,
            context,
            &model,
            parsed.usage,
            None,
            status_code,
            response_error_message(status_code, error_message_from_response_bytes(body)),
        )
        .await;
        return;
    }

    if !context.policy.logs_zero_usage_on_parse_failure(status_code) {
        return;
    }

    let model = fallback_model_from_response_bytes(body, &context.request_model);
    insert_request_log(
        state,
        context,
        &model,
        TokenUsage::default(),
        None,
        status_code,
        response_error_message(status_code, error_message_from_response_bytes(body)),
    )
    .await;
}

pub async fn log_stream_response(
    state: &ProxyServerState,
    context: &RequestLogContext,
    status_code: u16,
    collector: &StreamLogCollector,
) {
    if !logging_enabled(state).await {
        return;
    }

    if let Some(parsed) = collector.parsed_usage() {
        let model = non_empty_model(&parsed, &context.request_model);
        insert_request_log(
            state,
            context,
            &model,
            parsed.usage,
            collector.first_event_ms(),
            status_code,
            response_error_message(status_code, collector.error_message()),
        )
        .await;
        return;
    }

    if !context.policy.logs_zero_usage_on_parse_failure(status_code) {
        return;
    }

    let model = collector.fallback_model(&context.request_model);
    insert_request_log(
        state,
        context,
        &model,
        TokenUsage::default(),
        collector.first_event_ms(),
        status_code,
        response_error_message(status_code, collector.error_message()),
    )
    .await;
}

pub async fn log_error_request(
    state: &ProxyServerState,
    context: &RequestLogContext,
    error: &ProxyError,
) {
    if !logging_enabled(state).await {
        return;
    }

    insert_request_log(
        state,
        context,
        &context.request_model,
        TokenUsage::default(),
        None,
        error.status_code().as_u16(),
        Some(error.to_string()),
    )
    .await;
}

async fn logging_enabled(state: &ProxyServerState) -> bool {
    state.config.read().await.enable_logging
}

async fn insert_request_log(
    state: &ProxyServerState,
    context: &RequestLogContext,
    model: &str,
    usage: TokenUsage,
    first_token_ms: Option<u64>,
    status_code: u16,
    error_message: Option<String>,
) {
    let pricing_config =
        resolve_pricing_config(state.db.as_ref(), &context.app_type, &context.provider).await;
    let pricing_model = pricing_model(
        &context.request_model,
        model,
        &pricing_config.pricing_model_source,
    );
    let cost = calculate_cost(
        &usage,
        lookup_model_pricing(state.db.as_ref(), pricing_model).as_ref(),
        pricing_config.cost_multiplier,
    );
    let request_id = uuid::Uuid::new_v4().to_string();
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);

    let conn = match state.db.conn.lock() {
        Ok(conn) => conn,
        Err(error) => {
            log::warn!("record proxy request log failed to lock db: {error}");
            return;
        }
    };

    if let Err(error) = conn.execute(
        "INSERT INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
        rusqlite::params![
            request_id,
            &context.provider.id,
            context.app_type.as_str(),
            model,
            &context.request_model,
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_tokens,
            usage.cache_creation_tokens,
            cost.as_ref().map(|value| format_decimal(value.input_cost)).unwrap_or_else(|| "0".to_string()),
            cost.as_ref().map(|value| format_decimal(value.output_cost)).unwrap_or_else(|| "0".to_string()),
            cost.as_ref().map(|value| format_decimal(value.cache_read_cost)).unwrap_or_else(|| "0".to_string()),
            cost.as_ref().map(|value| format_decimal(value.cache_creation_cost)).unwrap_or_else(|| "0".to_string()),
            cost.as_ref().map(|value| format_decimal(value.total_cost)).unwrap_or_else(|| "0".to_string()),
            context.latency_ms() as i64,
            first_token_ms.map(|value| value as i64),
            status_code as i64,
            error_message,
            &context.session_id,
            Option::<String>::None,
            context.is_streaming as i64,
            &pricing_config.cost_multiplier_raw,
            created_at,
        ],
    ) {
        log::warn!("record proxy request log failed: {error}");
    }
}

fn response_error_message(status_code: u16, error_message: Option<String>) -> Option<String> {
    (status_code >= 400).then_some(error_message).flatten()
}

fn non_empty_model(parsed: &ParsedUsage, request_model: &str) -> String {
    if parsed.model.is_empty() {
        request_model.to_string()
    } else {
        parsed.model.clone()
    }
}
