use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::OptionalExtension;

use crate::{
    app_config::AppType,
    provider::Provider,
    proxy::{error::ProxyError, handler_context::HandlerContext, server::ProxyServerState},
    services::sql_helpers::{INPUT_TOKEN_SEMANTICS_FRESH, INPUT_TOKEN_SEMANTICS_TOTAL},
};

use super::{
    calculator::{
        calculate_cost, format_decimal, lookup_model_pricing, pricing_model, resolve_pricing_config,
    },
    parser::{
        error_message_from_response_bytes, fallback_model_from_response_bytes,
        parse_response_usage, ParsedUsage, StreamLogCollector, TokenUsage,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageLogPolicy {
    Passthrough,
    Transformed,
}

fn input_token_semantics_for_app(app_type: &AppType) -> i64 {
    if matches!(app_type.as_str(), "codex" | "gemini") {
        INPUT_TOKEN_SEMANTICS_TOTAL
    } else {
        INPUT_TOKEN_SEMANTICS_FRESH
    }
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
    pub session_client_provided: bool,
    pub(crate) session_id_encoding: crate::services::session_identity::ProxySessionIdEncoding,
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
            session_client_provided: context.session_client_provided,
            session_id_encoding: context.session_id_encoding,
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

    if let Some(parsed) = parse_response_usage(&context.app_type, body) {
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

    if let Some(parsed) = collector.parsed_usage_for_app(&context.app_type) {
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
        &context.app_type,
        &usage,
        lookup_model_pricing(state.db.as_ref(), pricing_model).as_ref(),
        pricing_config.cost_multiplier,
    );
    let request_id = usage.dedup_request_id();
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let input_token_semantics = input_token_semantics_for_app(&context.app_type);
    let mut session_storage = crate::services::session_identity::proxy_session_storage(
        &context.app_type,
        &context.session_id,
        context.session_client_provided,
        context.session_id_encoding,
    );

    let mut conn = match state.db.conn.lock() {
        Ok(conn) => conn,
        Err(error) => {
            log::warn!("record proxy request log failed to lock db: {error}");
            return;
        }
    };
    let tx = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(error) => {
            log::warn!("record proxy request log failed to start transaction: {error}");
            return;
        }
    };
    match imported_session_identity(
        &tx,
        &context.app_type,
        &request_id,
        context.session_client_provided,
    ) {
        Ok(Some(session_id)) => {
            session_storage = crate::services::session_identity::proxy_session_storage(
                &context.app_type,
                &session_id,
                true,
                crate::services::session_identity::ProxySessionIdEncoding::Native,
            );
        }
        Ok(None) => {}
        Err(error) => {
            log::warn!("recover imported session identity before proxy logging failed: {error}");
            return;
        }
    }

    if let Err(error) = tx.execute(
        "INSERT OR REPLACE INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model, pricing_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_token_semantics,
            input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at, data_source
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
        rusqlite::params![
            request_id,
            &context.provider.id,
            context.app_type.as_str(),
            model,
            &context.request_model,
            if cost.is_some() { pricing_model } else { "" },
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_tokens,
            usage.cache_creation_tokens,
            input_token_semantics,
            cost.as_ref().map(|value| format_decimal(value.input_cost)).unwrap_or_else(|| "0".to_string()),
            cost.as_ref().map(|value| format_decimal(value.output_cost)).unwrap_or_else(|| "0".to_string()),
            cost.as_ref().map(|value| format_decimal(value.cache_read_cost)).unwrap_or_else(|| "0".to_string()),
            cost.as_ref().map(|value| format_decimal(value.cache_creation_cost)).unwrap_or_else(|| "0".to_string()),
            cost.as_ref().map(|value| format_decimal(value.total_cost)).unwrap_or_else(|| "0".to_string()),
            context.latency_ms() as i64,
            first_token_ms.map(|value| value as i64),
            status_code as i64,
            error_message,
            &session_storage.session_id,
            session_storage.provider_type,
            context.is_streaming as i64,
            format_decimal(pricing_config.cost_multiplier),
            created_at,
            "proxy",
        ],
    ) {
        log::warn!("record proxy request log failed: {error}");
        return;
    }
    if (200..300).contains(&status_code) {
        match crate::services::session_usage::delete_session_logs_covered_by_proxy_log(
            &tx,
            context.app_type.as_str(),
            model,
            &usage,
            created_at,
        ) {
            Ok(deleted) if deleted > 0 => {
                log::debug!("removed {deleted} session usage log(s) covered by proxy log");
            }
            Ok(_) => {}
            Err(error) => log::warn!("deduplicate proxy/session usage logs failed: {error}"),
        }
    }
    if let Err(error) = tx.commit() {
        log::warn!("record proxy request log failed to commit: {error}");
    }
}

fn imported_session_identity(
    conn: &rusqlite::Connection,
    app_type: &AppType,
    request_id: &str,
    proxy_identity_is_trustworthy: bool,
) -> rusqlite::Result<Option<String>> {
    if proxy_identity_is_trustworthy {
        return Ok(None);
    }
    let expected_source = match app_type {
        AppType::Claude => "session_log",
        AppType::Codex => "codex_session",
        _ => return Ok(None),
    };
    let existing = conn
        .query_row(
            "SELECT session_id
             FROM proxy_request_logs
             WHERE request_id = ?1
               AND app_type = ?2
               AND COALESCE(data_source, 'proxy') = ?3
               AND NULLIF(TRIM(session_id), '') IS NOT NULL",
            rusqlite::params![request_id, app_type.as_str(), expected_source],
            |row| row.get::<_, String>(0),
        )
        .optional()?;

    Ok(existing)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_logs_mark_cache_inclusive_apps_as_total() {
        assert_eq!(
            input_token_semantics_for_app(&AppType::Codex),
            INPUT_TOKEN_SEMANTICS_TOTAL
        );
        assert_eq!(
            input_token_semantics_for_app(&AppType::Gemini),
            INPUT_TOKEN_SEMANTICS_TOTAL
        );
        assert_eq!(
            input_token_semantics_for_app(&AppType::Claude),
            INPUT_TOKEN_SEMANTICS_FRESH
        );
    }

    #[test]
    fn session_storage_preserves_raw_values_and_marks_stable_ids() {
        assert_eq!(
            crate::services::session_identity::proxy_session_storage(
                &AppType::Claude,
                "generated-request-id",
                false,
                crate::services::session_identity::ProxySessionIdEncoding::Native,
            )
            .session_id,
            "generated-request-id"
        );
        let codex = crate::services::session_identity::proxy_session_storage(
            &AppType::Codex,
            "client-session-id",
            true,
            crate::services::session_identity::ProxySessionIdEncoding::Native,
        );
        assert_eq!(codex.session_id, "client-session-id");
        assert_eq!(
            codex.provider_type,
            Some(crate::services::session_identity::CODEX_NATIVE_SESSION_PROVIDER_TYPE)
        );
        assert_eq!(
            crate::services::session_identity::proxy_session_storage(
                &AppType::Claude,
                "client-session-id",
                true,
                crate::services::session_identity::ProxySessionIdEncoding::Native,
            )
            .provider_type,
            Some(crate::services::session_identity::CLAUDE_STABLE_SESSION_PROVIDER_TYPE)
        );
    }

    #[test]
    fn exact_import_match_supplies_missing_proxy_session_identity() -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE proxy_request_logs (
                request_id TEXT PRIMARY KEY,
                app_type TEXT NOT NULL,
                session_id TEXT,
                data_source TEXT
            );
            INSERT INTO proxy_request_logs (request_id, app_type, session_id, data_source)
            VALUES ('session:message-1', 'claude', 'session-a', 'session_log');",
        )?;

        assert_eq!(
            imported_session_identity(&conn, &AppType::Claude, "session:message-1", false)?
                .as_deref(),
            Some("session-a")
        );
        assert_eq!(
            imported_session_identity(&conn, &AppType::Claude, "different-request", false)?,
            None
        );
        assert_eq!(
            imported_session_identity(&conn, &AppType::Claude, "session:message-1", true)?,
            None
        );
        Ok(())
    }
}
