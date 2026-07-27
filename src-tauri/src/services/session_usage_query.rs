//! Lightweight per-session usage projection over retained request logs.
//!
//! Session detail intentionally stays schema-free: daily rollups do not retain a
//! session dimension, so this query only reports rows still present in
//! `proxy_request_logs`.

use std::cmp::Ordering;

use chrono::{Local, NaiveDate, TimeZone};
use rusqlite::{params, Connection};
use serde::{Serialize, Serializer};

use crate::app_config::AppType;
use crate::error::AppError;
use crate::services::sql_helpers::fresh_input_sql;

pub const SESSION_USAGE_ROW_LIMIT: usize = 100;
pub const SESSION_USAGE_MODEL_LIST_LIMIT: usize = 16;
const SESSION_USAGE_MODEL_CHAR_LIMIT: usize = 256;
const SESSION_USAGE_MODEL_SUFFIX_CHARS: usize = 64;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsageRow {
    pub session_id: String,
    /// Bounded display value for the first model in binary sort order.
    pub model: String,
    /// At most [`SESSION_USAGE_MODEL_LIST_LIMIT`] bounded display values.
    /// `model_count` retains the full distinct count when this list is shorter.
    pub models: Vec<String>,
    pub model_count: u64,
    pub request_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    #[serde(serialize_with = "serialize_cost_usd")]
    pub total_cost_usd: f64,
    /// Unix timestamp in seconds.
    #[serde(serialize_with = "serialize_timestamp_ms")]
    pub last_active_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct SessionUsageQueryResult {
    pub rows: Vec<SessionUsageRow>,
    pub total_sessions: u64,
}

impl SessionUsageQueryResult {
    pub fn truncated(&self) -> bool {
        self.total_sessions > self.rows.len() as u64
    }
}

impl SessionUsageRow {
    pub fn compact_session_id(&self) -> String {
        compact_middle(&self.session_id, 12)
    }

    pub fn display_session_id(&self, max_chars: usize) -> String {
        compact_middle(&self.session_id, max_chars)
    }

    pub fn display_model_label(&self, max_chars: usize) -> String {
        let suffix = (self.model_count > 1).then(|| format!(" +{}", self.model_count - 1));
        let suffix_len = suffix.as_ref().map_or(0, |value| value.chars().count());
        let model_limit = max_chars.saturating_sub(suffix_len).max(5);
        format!(
            "{}{}",
            compact_middle(&self.model, model_limit),
            suffix.unwrap_or_default()
        )
    }

    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_creation_tokens)
    }

    pub fn cache_tokens(&self) -> u64 {
        self.cache_read_tokens
            .saturating_add(self.cache_creation_tokens)
    }
}

pub fn supports_session_usage(app_type: &AppType) -> bool {
    matches!(app_type, AppType::Claude | AppType::Codex)
}

pub(crate) fn local_day_start_timestamp(date: NaiveDate) -> Option<i64> {
    first_valid_local_time(date, resolve_local_timestamp)
}

pub(crate) fn local_day_end_timestamp(date: NaiveDate) -> Option<i64> {
    last_valid_local_time(date, resolve_local_timestamp)
}

fn resolve_local_timestamp(naive: chrono::NaiveDateTime) -> chrono::LocalResult<i64> {
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(datetime) => chrono::LocalResult::Single(datetime.timestamp()),
        chrono::LocalResult::Ambiguous(earliest, latest) => {
            chrono::LocalResult::Ambiguous(earliest.timestamp(), latest.timestamp())
        }
        chrono::LocalResult::None => chrono::LocalResult::None,
    }
}

fn first_valid_local_time<T>(
    date: NaiveDate,
    mut resolve: impl FnMut(chrono::NaiveDateTime) -> chrono::LocalResult<T>,
) -> Option<T> {
    let midnight = date.and_hms_opt(0, 0, 0)?;
    for second in 0..86_400 {
        let candidate = midnight.checked_add_signed(chrono::Duration::seconds(second))?;
        match resolve(candidate) {
            chrono::LocalResult::Single(value) | chrono::LocalResult::Ambiguous(value, _) => {
                return Some(value)
            }
            chrono::LocalResult::None => {}
        }
    }
    None
}

fn last_valid_local_time<T>(
    date: NaiveDate,
    mut resolve: impl FnMut(chrono::NaiveDateTime) -> chrono::LocalResult<T>,
) -> Option<T> {
    let midnight = date.and_hms_opt(0, 0, 0)?;
    for second in (0..86_400).rev() {
        let candidate = midnight.checked_add_signed(chrono::Duration::seconds(second))?;
        match resolve(candidate) {
            chrono::LocalResult::Single(value) | chrono::LocalResult::Ambiguous(_, value) => {
                return Some(value)
            }
            chrono::LocalResult::None => {}
        }
    }
    None
}

fn compact_middle(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars || max_chars < 5 {
        return value.to_string();
    }

    let suffix_chars = (max_chars / 3).max(2);
    let prefix_chars = max_chars.saturating_sub(suffix_chars + 1);
    let prefix = value.chars().take(prefix_chars).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(suffix_chars)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix}…{suffix}")
}

#[derive(Debug)]
struct StoredUsageLog {
    session_id: String,
    model: String,
    model_position: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    fresh_input_tokens: u64,
    total_cost_usd: f64,
    created_at: i64,
}

#[derive(Debug, Default)]
struct SessionAggregate {
    models: Vec<String>,
    model_count: u64,
    last_model_position: Option<u64>,
    request_count: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    total_cost_usd: f64,
    last_active_at: i64,
}

pub fn query_session_usage(
    conn: &Connection,
    app_type: &AppType,
    start: i64,
    end: i64,
) -> Result<SessionUsageQueryResult, AppError> {
    let mut results = query_session_usage_ranges(conn, app_type, &[(start, end)])?;
    Ok(results.pop().unwrap_or_default())
}

/// Query multiple inclusive ranges with one retained-log scan.
///
/// The fixed TUI snapshot requests Today, 7d, and 30d together. Reading the
/// widest range once keeps the shared dedup projection from running three
/// times while preserving the same per-range result.
pub fn query_session_usage_ranges(
    conn: &Connection,
    app_type: &AppType,
    ranges: &[(i64, i64)],
) -> Result<Vec<SessionUsageQueryResult>, AppError> {
    if !supports_session_usage(app_type) {
        return Err(AppError::InvalidInput(format!(
            "session usage is only available for claude and codex; got {}",
            app_type.as_str()
        )));
    }
    for (start, end) in ranges {
        if end < start {
            return Err(AppError::InvalidInput(
                "session usage range end must not precede its start".to_string(),
            ));
        }
    }
    if ranges.is_empty() {
        return Ok(Vec::new());
    }
    let query_start = ranges.iter().map(|(start, _)| *start).min().unwrap_or(0);
    let query_end = ranges.iter().map(|(_, end)| *end).max().unwrap_or(0);

    let fresh_input = fresh_input_sql("l");
    let data_source = "COALESCE(l.data_source, 'proxy')";
    let stable_proxy = crate::services::session_identity::stable_proxy_session_sql("l", app_type);
    let normalized_session = crate::services::usage_stats::normalized_session_sql("l");
    let codex_evidence = crate::services::session_identity::codex_imported_session_keys_cte_sql(
        matches!(app_type, AppType::Codex),
    );
    let normalized_model = "COALESCE(NULLIF(TRIM(l.model), ''), 'unknown')";
    let model_prefix_chars = SESSION_USAGE_MODEL_CHAR_LIMIT - SESSION_USAGE_MODEL_SUFFIX_CHARS - 1;
    let bounded_model = format!(
        "CASE
            WHEN length({normalized_model}) > {SESSION_USAGE_MODEL_CHAR_LIMIT}
            THEN substr({normalized_model}, 1, {model_prefix_chars})
                 || '…'
                 || substr({normalized_model}, -{SESSION_USAGE_MODEL_SUFFIX_CHARS})
            ELSE {normalized_model}
         END"
    );
    let effective_filter =
        crate::services::usage_stats::effective_usage_log_filter_for_app_connection(
            conn,
            "l",
            app_type.as_str(),
        )?;
    let sql = format!(
        "WITH {codex_evidence}
        SELECT
            normalized_session_id,
            model,
            model_position,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
            fresh_input_tokens,
            total_cost_usd,
            created_at
        FROM (
            SELECT
                {normalized_session} AS normalized_session_id,
                {bounded_model} AS model,
                DENSE_RANK() OVER (
                    PARTITION BY {normalized_session}
                    ORDER BY {normalized_model} COLLATE BINARY
                ) AS model_position,
                l.output_tokens,
                l.cache_read_tokens,
                l.cache_creation_tokens,
                {fresh_input} AS fresh_input_tokens,
                CAST(l.total_cost_usd AS REAL) AS total_cost_usd,
                l.created_at,
                l.request_id
            FROM proxy_request_logs l
            WHERE l.app_type = ?1
              AND l.created_at >= ?2
              AND l.created_at <= ?3
              AND NULLIF(TRIM(l.session_id), '') IS NOT NULL
              AND NULLIF({normalized_session}, '') IS NOT NULL
              AND (
                  ({data_source} = 'proxy' AND {stable_proxy})
                  OR (l.app_type = 'claude' AND {data_source} = 'session_log')
                  OR (l.app_type = 'codex' AND {data_source} = 'codex_session')
              )
              AND {effective_filter}
        ) retained_session_logs
        ORDER BY normalized_session_id COLLATE BINARY,
                 model_position ASC,
                 created_at ASC,
                 request_id ASC"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![app_type.as_str(), query_start, query_end], |row| {
        Ok(StoredUsageLog {
            session_id: row.get(0)?,
            model: row.get(1)?,
            model_position: non_negative_u64(row.get(2)?),
            output_tokens: non_negative_u64(row.get(3)?),
            cache_read_tokens: non_negative_u64(row.get(4)?),
            cache_creation_tokens: non_negative_u64(row.get(5)?),
            fresh_input_tokens: non_negative_u64(row.get(6)?),
            total_cost_usd: row.get(7)?,
            created_at: row.get(8)?,
        })
    })?;

    let mut results = ranges
        .iter()
        .map(|_| BoundedSessionRows::default())
        .collect::<Vec<_>>();
    let mut current_session: Option<String> = None;
    let mut current_aggregates = ranges
        .iter()
        .map(|_| None)
        .collect::<Vec<Option<SessionAggregate>>>();

    for row in rows {
        let log = row?;
        if current_session.as_deref() != Some(log.session_id.as_str()) {
            if let Some(session_id) = current_session.take() {
                flush_session(&session_id, &mut current_aggregates, &mut results);
            }
            current_session = Some(log.session_id.clone());
        }

        for ((start, end), aggregate) in ranges.iter().zip(current_aggregates.iter_mut()) {
            if log.created_at < *start || log.created_at > *end {
                continue;
            }
            aggregate
                .get_or_insert_with(|| SessionAggregate {
                    last_active_at: log.created_at,
                    ..SessionAggregate::default()
                })
                .add(&log);
        }
    }
    if let Some(session_id) = current_session {
        flush_session(&session_id, &mut current_aggregates, &mut results);
    }

    Ok(results
        .into_iter()
        .map(BoundedSessionRows::finish)
        .collect())
}

impl SessionAggregate {
    fn add(&mut self, log: &StoredUsageLog) {
        if self.last_model_position != Some(log.model_position) {
            self.last_model_position = Some(log.model_position);
            self.model_count = self.model_count.saturating_add(1);
            if self.models.len() < SESSION_USAGE_MODEL_LIST_LIMIT {
                self.models.push(log.model.clone());
            }
        }
        let total_cost_usd = if log.total_cost_usd.is_finite() {
            log.total_cost_usd.max(0.0)
        } else {
            0.0
        };
        self.request_count = self.request_count.saturating_add(1);
        self.input_tokens = self.input_tokens.saturating_add(log.fresh_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(log.output_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(log.cache_read_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(log.cache_creation_tokens);
        self.total_cost_usd += total_cost_usd;
        self.last_active_at = self.last_active_at.max(log.created_at);
    }
}

#[derive(Default)]
struct BoundedSessionRows {
    rows: Vec<SessionUsageRow>,
    total_sessions: u64,
}

impl BoundedSessionRows {
    fn push(&mut self, row: SessionUsageRow) {
        self.total_sessions = self.total_sessions.saturating_add(1);
        self.rows.push(row);
        self.rows.sort_by(compare_session_rows);
        self.rows.truncate(SESSION_USAGE_ROW_LIMIT);
    }

    fn finish(mut self) -> SessionUsageQueryResult {
        self.rows.sort_by(compare_session_rows);
        SessionUsageQueryResult {
            rows: self.rows,
            total_sessions: self.total_sessions,
        }
    }
}

fn flush_session(
    session_id: &str,
    aggregates: &mut [Option<SessionAggregate>],
    results: &mut [BoundedSessionRows],
) {
    for (aggregate, result) in aggregates.iter_mut().zip(results.iter_mut()) {
        let Some(aggregate) = aggregate.take() else {
            continue;
        };
        let models = aggregate.models.into_iter().collect::<Vec<_>>();
        result.push(SessionUsageRow {
            session_id: session_id.to_string(),
            model: models
                .first()
                .cloned()
                .unwrap_or_else(|| "unknown".to_string()),
            model_count: aggregate.model_count,
            models,
            request_count: aggregate.request_count,
            input_tokens: aggregate.input_tokens,
            output_tokens: aggregate.output_tokens,
            cache_read_tokens: aggregate.cache_read_tokens,
            cache_creation_tokens: aggregate.cache_creation_tokens,
            total_cost_usd: if aggregate.total_cost_usd.is_finite() {
                aggregate.total_cost_usd.max(0.0)
            } else {
                0.0
            },
            last_active_at: aggregate.last_active_at.max(0),
        });
    }
}

fn compare_session_rows(left: &SessionUsageRow, right: &SessionUsageRow) -> Ordering {
    right
        .last_active_at
        .cmp(&left.last_active_at)
        .then_with(|| left.session_id.cmp(&right.session_id))
}

fn non_negative_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn serialize_cost_usd<S>(value: &f64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&format!("{value:.6}"))
}

fn serialize_timestamp_ms<S>(value: &i64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_i64(value.saturating_mul(1_000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory database");
        conn.execute_batch(
            "CREATE TABLE proxy_request_logs (
                request_id TEXT PRIMARY KEY,
                app_type TEXT NOT NULL,
                model TEXT NOT NULL,
                pricing_model TEXT NOT NULL DEFAULT '',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                input_token_semantics INTEGER NOT NULL DEFAULT 0,
                total_cost_usd TEXT NOT NULL DEFAULT '0',
                status_code INTEGER NOT NULL DEFAULT 200,
                session_id TEXT,
                provider_type TEXT,
                created_at INTEGER NOT NULL,
                data_source TEXT NOT NULL DEFAULT 'proxy'
            );
            CREATE TABLE usage_daily_rollups (
                date TEXT NOT NULL,
                app_type TEXT NOT NULL,
                PRIMARY KEY (date, app_type)
            );",
        )
        .expect("create request log table");
        conn
    }

    #[test]
    fn local_day_start_advances_past_a_midnight_gap() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 27).expect("valid date");
        let resolved = first_valid_local_time(date, |candidate| {
            if candidate.time().hour() == 0 {
                chrono::LocalResult::None
            } else {
                chrono::LocalResult::Single(candidate)
            }
        })
        .expect("the first valid local instant");

        assert_eq!(
            resolved,
            date.and_hms_opt(1, 0, 0).expect("valid one o'clock")
        );
    }

    #[test]
    fn local_day_end_retreats_before_an_end_of_day_gap() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 27).expect("valid date");
        let resolved = last_valid_local_time(date, |candidate| {
            if candidate.time().hour() == 23 {
                chrono::LocalResult::None
            } else {
                chrono::LocalResult::Single(candidate)
            }
        })
        .expect("the last valid local instant");

        assert_eq!(
            resolved,
            date.and_hms_opt(22, 59, 59)
                .expect("valid last instant before the gap")
        );
    }

    #[test]
    fn local_day_boundaries_reject_a_fully_missing_day() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 27).expect("valid date");
        assert!(first_valid_local_time(date, |_| {
            chrono::LocalResult::<chrono::NaiveDateTime>::None
        })
        .is_none());
        assert!(last_valid_local_time(date, |_| {
            chrono::LocalResult::<chrono::NaiveDateTime>::None
        })
        .is_none());
    }

    fn effective_request_count(conn: &Connection, app_type: &str) -> Result<i64, AppError> {
        let filter = crate::services::usage_stats::effective_usage_log_filter("l");
        let sql = format!(
            "SELECT COUNT(*)
             FROM proxy_request_logs l
             WHERE l.app_type = ?1 AND {filter}"
        );
        Ok(conn.query_row(&sql, [app_type], |row| row.get(0))?)
    }

    #[test]
    fn groups_codex_transport_and_native_session_ids() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                cache_read_tokens, input_token_semantics, total_cost_usd,
                session_id, created_at, data_source
            ) VALUES
                ('proxy', 'codex', 'gpt-5.2', 100, 10, 20, 0, '0.1',
                 'codex_thread-1', 20, 'proxy'),
                ('transcript', 'codex', 'gpt-5.1', 50, 5, 0, 0, '0.2',
                 'thread-1', 10, 'codex_session')",
            [],
        )?;

        let result = query_session_usage(&conn, &AppType::Codex, 0, 30)?;

        assert_eq!(result.total_sessions, 1);
        assert!(!result.truncated());
        let row = &result.rows[0];
        assert_eq!(row.session_id, "thread-1");
        assert_eq!(row.request_count, 2);
        assert_eq!(row.model_count, 2);
        assert_eq!(row.display_model_label(32), "gpt-5.1 +1");
        assert_eq!(row.models, vec!["gpt-5.1", "gpt-5.2"]);
        assert_eq!(row.input_tokens, 130);
        assert_eq!(row.output_tokens, 15);
        assert_eq!(row.cache_read_tokens, 20);
        assert_eq!(row.total_tokens(), 165);
        assert!((row.total_cost_usd - 0.3).abs() < f64::EPSILON);
        assert_eq!(row.last_active_at, 20);
        Ok(())
    }

    #[test]
    fn batches_fixed_ranges_from_one_retained_log_read() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('old', 'claude', 'claude-sonnet', 1, 1, '0.1',
                 'session-a', 10, 'proxy'),
                ('middle', 'claude', 'claude-sonnet', 2, 1, '0.1',
                 'session-a', 50, 'proxy'),
                ('recent', 'claude', 'claude-sonnet', 3, 1, '0.1',
                 'session-a', 100, 'proxy');",
        )?;

        let results =
            query_session_usage_ranges(&conn, &AppType::Claude, &[(80, 120), (40, 120), (0, 120)])?;

        assert_eq!(results.len(), 3);
        assert_eq!(
            results
                .iter()
                .map(|result| result.rows[0].request_count)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            results
                .iter()
                .map(|result| result.rows[0].input_tokens)
                .collect::<Vec<_>>(),
            vec![3, 5, 6]
        );
        Ok(())
    }

    #[test]
    fn filters_app_range_blank_sessions_and_existing_cross_source_duplicates(
    ) -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                cache_read_tokens, cache_creation_tokens, total_cost_usd,
                session_id, created_at, data_source
            ) VALUES
                ('claude-proxy', 'claude', 'claude-sonnet', 10, 2, 0, 0, '0.1',
                 'session-a', 100, 'proxy'),
                ('claude-duplicate', 'claude', 'claude-sonnet', 10, 2, 0, 0, '0.1',
                 'session-a', 100, 'session_log'),
                ('outside-range', 'claude', 'claude-sonnet', 1, 1, 0, 0, '0.1',
                 'old', 1, 'proxy'),
                ('blank-session', 'claude', 'claude-sonnet', 1, 1, 0, 0, '0.1',
                 '  ', 100, 'proxy'),
                ('other-app', 'codex', 'gpt-5.2', 1, 1, 0, 0, '0.1',
                 'codex_other', 100, 'proxy');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 50, 150)?;

        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].session_id, "session-a");
        assert_eq!(result.rows[0].request_count, 1);
        Ok(())
    }

    #[test]
    fn reports_total_when_recent_rows_are_bounded() -> Result<(), AppError> {
        let mut conn = setup_conn();
        let tx = conn.transaction()?;
        for index in 0..=SESSION_USAGE_ROW_LIMIT {
            tx.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, app_type, model, input_tokens, output_tokens,
                    total_cost_usd, session_id, created_at
                ) VALUES (?1, 'claude', 'claude-sonnet', 1, 1, '0.1', ?2, ?3)",
                params![
                    format!("request-{index}"),
                    format!("session-{index}"),
                    index as i64
                ],
            )?;
        }
        tx.commit()?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, i64::MAX)?;

        assert_eq!(result.rows.len(), SESSION_USAGE_ROW_LIMIT);
        assert_eq!(result.total_sessions, (SESSION_USAGE_ROW_LIMIT + 1) as u64);
        assert!(result.truncated());
        assert_eq!(result.rows[0].session_id, "session-100");
        Ok(())
    }

    #[test]
    fn bounds_models_per_session_but_keeps_the_distinct_count() -> Result<(), AppError> {
        let mut conn = setup_conn();
        let tx = conn.transaction()?;
        let distinct_models = SESSION_USAGE_MODEL_LIST_LIMIT + 9;
        for index in 0..distinct_models {
            let long_middle = "x".repeat(SESSION_USAGE_MODEL_CHAR_LIMIT * 2);
            let model = format!("model-{index:03}-{long_middle}-tail-{index:03}");
            tx.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, app_type, model, input_tokens, output_tokens,
                    total_cost_usd, session_id, created_at
                ) VALUES (?1, 'claude', ?2, 1, 1, '0.1', 'one-session', ?3)",
                params![format!("request-{index}"), model, index as i64],
            )?;
        }
        tx.commit()?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, i64::MAX)?;
        let row = &result.rows[0];

        assert_eq!(row.model_count, distinct_models as u64);
        assert_eq!(row.models.len(), SESSION_USAGE_MODEL_LIST_LIMIT);
        assert!(row.models.iter().all(|model| model.chars().count()
            <= SESSION_USAGE_MODEL_CHAR_LIMIT
            && model.contains('…')));
        assert!(row.models[0].ends_with("tail-000"));
        assert_eq!(row.request_count, distinct_models as u64);
        Ok(())
    }

    #[test]
    fn accepts_conservative_historical_claude_identity_proofs() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('generated', 'claude', 'claude-sonnet', 1, 1, '0.1',
                 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee', 100, 'proxy'),
                ('historical-native', 'claude', 'claude-sonnet', 10, 2, '0.2',
                 'historical-real', 110, 'proxy'),
                ('uuid-proxy', 'claude', 'claude-sonnet', 20, 4, '0.4',
                 'bbbbbbbb-cccc-4ddd-8eee-ffffffffffff', 120, 'proxy'),
                ('uuid-proof', 'claude', 'claude-sonnet', 20, 4, '0.4',
                 'bbbbbbbb-cccc-4ddd-8eee-ffffffffffff', 120, 'session_log');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, 200)?;

        assert_eq!(result.total_sessions, 2);
        assert!(result
            .rows
            .iter()
            .all(|row| row.session_id != "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"));
        assert!(result
            .rows
            .iter()
            .any(|row| row.session_id == "historical-real" && row.request_count == 1));
        Ok(())
    }

    #[test]
    fn marked_proxy_keeps_historical_session_proof_after_transcript_cleanup() -> Result<(), AppError>
    {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, provider_type, created_at, data_source
            ) VALUES
                ('historical-proxy', 'claude', 'claude-sonnet', 10, 2, '0.2',
                 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee', NULL, 100, 'proxy'),
                ('transcript-proof', 'claude', 'claude-sonnet', 1, 1, '0.1',
                 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee', 'session_log', 90, 'session_log'),
                ('marked-proxy', 'claude', 'claude-sonnet', 3, 1, '0.1',
                 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee',
                 'ccswitch:claude-session', 110, 'proxy');
             DELETE FROM proxy_request_logs WHERE request_id = 'transcript-proof';",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, 200)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(
            result.rows[0].session_id,
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
        );
        assert_eq!(result.rows[0].request_count, 2);
        Ok(())
    }

    #[test]
    fn keeps_identical_fingerprints_from_different_stable_sessions() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy-a', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'session-a', 100, 'proxy'),
                ('transcript-b', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'session-b', 100, 'session_log');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, 200)?;

        assert_eq!(result.total_sessions, 2);
        assert_eq!(effective_request_count(&conn, "claude")?, 2);
        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| row.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["session-a", "session-b"]
        );
        Ok(())
    }

    #[test]
    fn cross_source_dedup_is_one_to_one() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'session-a', 100, 'proxy'),
                ('import-1', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'session-a', 100, 'session_log'),
                ('import-2', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'session-a', 101, 'session_log');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, 200)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].request_count, 2);
        assert_eq!(effective_request_count(&conn, "claude")?, 2);
        assert_eq!(result.rows[0].input_tokens, 20);
        assert_eq!(result.rows[0].output_tokens, 4);
        assert!((result.rows[0].total_cost_usd - 0.2).abs() < f64::EPSILON);
        Ok(())
    }

    #[test]
    fn window_orphan_does_not_shift_a_later_cross_source_pair() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('import-orphan', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'session-a', 100, 'session_log'),
                ('proxy-pair', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'session-a', 1000, 'proxy'),
                ('import-pair', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'session-a', 1000, 'session_log');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, 2000)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].request_count, 2);
        assert_eq!(effective_request_count(&conn, "claude")?, 2);
        Ok(())
    }

    #[test]
    fn cross_midnight_pairs_count_once_in_either_arrival_order() -> Result<(), AppError> {
        let conn = setup_conn();
        let midnight = match chrono::Local.with_ymd_and_hms(2026, 7, 26, 0, 0, 0) {
            chrono::LocalResult::Single(value) => value.timestamp(),
            chrono::LocalResult::Ambiguous(earliest, _) => earliest.timestamp(),
            chrono::LocalResult::None => panic!("test midnight must exist"),
        };
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy-before', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'codex_thread-a', ?1, 'proxy'),
                ('import-after', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'thread-a', ?2, 'codex_session'),
                ('import-before', 'codex', 'gpt-5.4', 20, 3, '0.2',
                 'thread-b', ?1, 'codex_session'),
                ('proxy-after', 'codex', 'gpt-5.4', 20, 3, '0.2',
                 'codex_thread-b', ?2, 'proxy')",
            params![midnight - 60, midnight + 60],
        )?;

        let result = query_session_usage(&conn, &AppType::Codex, midnight - 120, midnight + 120)?;

        assert_eq!(result.total_sessions, 2);
        assert!(result.rows.iter().all(|row| row.request_count == 1));
        assert_eq!(effective_request_count(&conn, "codex")?, 2);
        assert!(result
            .rows
            .iter()
            .any(|row| row.session_id == "thread-a"
                && (row.total_cost_usd - 0.1).abs() < f64::EPSILON));
        assert!(result
            .rows
            .iter()
            .any(|row| row.session_id == "thread-b"
                && (row.total_cost_usd - 0.2).abs() < f64::EPSILON));
        Ok(())
    }

    #[test]
    fn non_uuid_session_ids_remain_case_sensitive() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'Run-A', 100, 'proxy'),
                ('import', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'run-a', 100, 'session_log');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, 200)?;

        assert_eq!(result.total_sessions, 2);
        assert_eq!(effective_request_count(&conn, "claude")?, 2);
        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| row.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["Run-A", "run-a"]
        );
        Ok(())
    }

    #[test]
    fn uuid_session_ids_match_case_insensitively_across_sources() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'codex_AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE', 100, 'proxy'),
                ('import', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee', 100, 'codex_session');",
        )?;

        let result = query_session_usage(&conn, &AppType::Codex, 0, 200)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].request_count, 1);
        assert_eq!(effective_request_count(&conn, "codex")?, 1);
        Ok(())
    }

    #[test]
    fn historical_claude_uuid_proof_is_case_insensitive() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE', 100, 'proxy'),
                ('import', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee', 100, 'session_log');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, 200)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].request_count, 1);
        assert_eq!(effective_request_count(&conn, "claude")?, 1);
        Ok(())
    }

    #[test]
    fn keeps_same_fingerprint_outside_dedup_window() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'session-a', 100, 'proxy'),
                ('import-next-day', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'session-a', 86500, 'session_log');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, 90_000)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].request_count, 2);
        assert_eq!(effective_request_count(&conn, "claude")?, 2);
        assert_eq!(result.rows[0].input_tokens, 20);
        assert_eq!(result.rows[0].output_tokens, 4);
        assert!((result.rows[0].total_cost_usd - 0.2).abs() < f64::EPSILON);
        Ok(())
    }

    #[test]
    fn does_not_cross_source_deduplicate_failures_or_zero_usage() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, status_code, session_id, created_at, data_source
            ) VALUES
                ('failed-proxy', 'claude', 'claude-sonnet', 10, 2, '0.1', 500,
                 'failed-session', 100, 'proxy'),
                ('failed-import-pair', 'claude', 'claude-sonnet', 10, 2, '0.1', 200,
                 'failed-session', 100, 'session_log'),
                ('zero-proxy', 'claude', 'claude-sonnet', 0, 0, '0', 200,
                 'zero-session', 100, 'proxy'),
                ('zero-import', 'claude', 'claude-sonnet', 0, 0, '0', 200,
                 'zero-session', 100, 'session_log');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, 200)?;

        assert_eq!(result.total_sessions, 2);
        assert!(result.rows.iter().all(|row| row.request_count == 2));
        Ok(())
    }

    #[test]
    fn unknown_model_does_not_match_a_named_model() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'codex_thread-a', 100, 'proxy'),
                ('import', 'codex', 'unknown', 10, 2, '0.1',
                 'thread-a', 100, 'codex_session');",
        )?;

        let result = query_session_usage(&conn, &AppType::Codex, 0, 200)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].request_count, 2);
        assert_eq!(effective_request_count(&conn, "codex")?, 2);
        assert_eq!(result.rows[0].models, vec!["gpt-5.4", "unknown"]);
        Ok(())
    }

    #[test]
    fn canonical_model_groups_match_independently() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy-unknown', 'codex', 'unknown', 10, 2, '0.1',
                 'codex_thread-a', 0, 'proxy'),
                ('proxy-a', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'codex_thread-a', 1200, 'proxy'),
                ('import-a', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'thread-a', 600, 'codex_session'),
                ('import-unknown', 'codex', 'unknown', 10, 2, '0.1',
                 'thread-a', 1800, 'codex_session');",
        )?;

        let result = query_session_usage(&conn, &AppType::Codex, 0, 2000)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].request_count, 3);
        assert_eq!(effective_request_count(&conn, "codex")?, 3);
        assert_eq!(result.rows[0].input_tokens, 30);
        Ok(())
    }

    #[test]
    fn range_boundary_uses_outside_proxy_only_for_deduplication() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy-before-range', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'session-a', 999, 'proxy'),
                ('import-in-range', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'session-a', 1001, 'session_log');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 1000, 2000)?;

        assert_eq!(result.total_sessions, 0);
        assert!(result.rows.is_empty());
        Ok(())
    }

    #[test]
    fn canonical_model_aliases_deduplicate_codex_sources() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy', 'codex', 'openai/gpt-5.4', 10, 2, '0.1',
                 'codex_thread-a', 100, 'proxy'),
                ('import', 'codex', 'gpt-5.4-2026-03-05', 10, 2, '0.1',
                 'thread-a', 100, 'codex_session');",
        )?;

        let result = query_session_usage(&conn, &AppType::Codex, 0, 200)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].request_count, 1);
        assert_eq!(result.rows[0].model, "openai/gpt-5.4");
        assert_eq!(result.rows[0].models, vec!["openai/gpt-5.4"]);
        Ok(())
    }

    #[test]
    fn codex_model_aliases_reuse_compact_date_importer_rule() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy', 'codex', 'azure/GPT-5.4-20260305', 10, 2, '0.1',
                 'codex_thread-a', 100, 'proxy'),
                ('import', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'thread-a', 100, 'codex_session');",
        )?;

        let result = query_session_usage(&conn, &AppType::Codex, 0, 200)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].request_count, 1);
        assert_eq!(result.rows[0].model, "azure/GPT-5.4-20260305");
        assert_eq!(result.rows[0].models, vec!["azure/GPT-5.4-20260305"]);
        Ok(())
    }

    #[test]
    fn claude_uses_actual_model_and_keeps_compact_date_suffix() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, pricing_model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy', 'claude', 'anthropic/Claude-Sonnet-4-20260206',
                 'unrelated-pricing-override', 10, 2, '0.1',
                 'session-a', 100, 'proxy'),
                ('import', 'claude', 'claude-sonnet-4-20260206',
                 'another-pricing-override', 10, 2, '0.1',
                 'session-a', 100, 'session_log');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, 200)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].request_count, 1);
        assert_eq!(result.rows[0].model, "anthropic/Claude-Sonnet-4-20260206");
        assert_eq!(
            result.rows[0].models,
            vec!["anthropic/Claude-Sonnet-4-20260206"]
        );
        Ok(())
    }

    #[test]
    fn different_model_families_with_same_tokens_do_not_deduplicate() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, provider_type, created_at, data_source
            ) VALUES
                ('proxy', 'claude', 'claude-opus-4-6', 10, 2, '0.1',
                 'session-a', 'ccswitch:claude-session', 100, 'proxy'),
                ('import', 'claude', 'claude-haiku-4-5', 10, 2, '0.1',
                 'session-a', NULL, 100, 'session_log');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, 200)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].request_count, 2);
        assert_eq!(effective_request_count(&conn, "claude")?, 2);
        assert_eq!(
            result.rows[0].models,
            vec!["claude-haiku-4-5", "claude-opus-4-6"]
        );
        Ok(())
    }

    #[test]
    fn claude_model_normalization_does_not_strip_dates() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, provider_type, created_at, data_source
            ) VALUES
                ('proxy', 'claude',
                 'Anthropic/CLAUDE-OPUS-4-6-20260205', 10, 2, '0.1',
                 'session-a', 'ccswitch:claude-session', 100, 'proxy'),
                ('import', 'claude',
                 'claude-opus-4-6', 10, 2, '0.1',
                 'session-a', NULL, 100, 'session_log');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, 200)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].request_count, 2);
        assert_eq!(effective_request_count(&conn, "claude")?, 2);
        Ok(())
    }

    #[test]
    fn imported_codex_native_prefix_is_not_treated_as_transport() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'codex_thread-a', 100, 'proxy'),
                ('import', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'codex_thread-a', 100, 'codex_session');",
        )?;

        let result = query_session_usage(&conn, &AppType::Codex, 0, 200)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].session_id, "codex_thread-a");
        assert_eq!(result.rows[0].request_count, 1);
        assert_eq!(effective_request_count(&conn, "codex")?, 1);
        Ok(())
    }

    #[test]
    fn backfilled_codex_native_prefix_round_trips_one_transport_layer() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee', 101, 'proxy'),
                ('import', 'codex', 'gpt-5.3', 9, 1, '0.1',
                 'codex_thread-a', 100, 'codex_session');",
        )?;

        assert!(
            crate::services::session_identity::attach_imported_session_identity(
                &conn,
                &AppType::Codex,
                "proxy",
                "codex_thread-a",
            )?
        );
        let stored = conn.query_row(
            "SELECT session_id, provider_type
             FROM proxy_request_logs
             WHERE request_id = 'proxy'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        assert_eq!(
            stored,
            (
                "codex_thread-a".to_string(),
                crate::services::session_identity::CODEX_NATIVE_SESSION_PROVIDER_TYPE.to_string()
            )
        );

        let result = query_session_usage(&conn, &AppType::Codex, 0, 200)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].session_id, "codex_thread-a");
        assert_eq!(result.rows[0].request_count, 2);
        Ok(())
    }

    #[test]
    fn ambiguous_or_unproven_legacy_codex_ids_are_not_guessed() -> Result<(), AppError> {
        let ambiguous = setup_conn();
        ambiguous.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'codex_thread-a', 100, 'proxy'),
                ('raw-import', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'codex_thread-a', 100, 'codex_session'),
                ('stripped-import', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'thread-a', 100, 'codex_session');",
        )?;

        let result = query_session_usage(&ambiguous, &AppType::Codex, 0, 200)?;
        assert_eq!(result.total_sessions, 2);
        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| row.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["codex_thread-a", "thread-a"]
        );
        assert_eq!(effective_request_count(&ambiguous, "codex")?, 3);

        let unproven = setup_conn();
        unproven.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, provider_type, created_at, data_source
            ) VALUES
                ('proxy', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'codex_unknown', NULL, 100, 'proxy'),
                ('marked-native', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'codex_native', 'ccswitch:codex-session-native', 100, 'proxy'),
                ('marked-transport', 'codex', 'gpt-5.4', 10, 2, '0.1',
                 'codex_thread', 'ccswitch:codex-session-transport', 100, 'proxy');",
        )?;
        let result = query_session_usage(&unproven, &AppType::Codex, 0, 200)?;
        assert_eq!(result.total_sessions, 2);
        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| row.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["codex_native", "thread"]
        );
        assert_eq!(effective_request_count(&unproven, "codex")?, 3);
        Ok(())
    }

    #[test]
    fn transcript_identity_wins_when_proxy_has_no_stable_session() -> Result<(), AppError> {
        let conn = setup_conn();
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('proxy-without-session', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 NULL, 100, 'proxy'),
                ('transcript', 'claude', 'claude-sonnet', 10, 2, '0.1',
                 'session-a', 100, 'session_log');",
        )?;

        let result = query_session_usage(&conn, &AppType::Claude, 0, 200)?;

        assert_eq!(result.total_sessions, 1);
        assert_eq!(result.rows[0].session_id, "session-a");
        assert_eq!(result.rows[0].request_count, 1);
        Ok(())
    }

    #[test]
    fn canonicalizes_codex_uuid_case_and_bounds_json_model_values() -> Result<(), AppError> {
        let conn = setup_conn();
        let long_suffix = "x".repeat(300);
        let long_id = format!("long-{long_suffix}");
        let long_model = format!("custom-model-{long_suffix}");
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES ('long', 'claude', ?1, 1, 1, '0.1', ?2, 100, 'proxy')",
            params![long_model, long_id],
        )?;
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                total_cost_usd, session_id, created_at, data_source
            ) VALUES
                ('codex-proxy', 'codex', 'gpt-5.2', 2, 1, '0.1',
                 'codex_AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE', 100, 'proxy'),
                ('codex-transcript', 'codex', 'gpt-5.1', 3, 1, '0.1',
                 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee', 90, 'codex_session');",
        )?;

        let claude = query_session_usage(&conn, &AppType::Claude, 0, 200)?;
        let codex = query_session_usage(&conn, &AppType::Codex, 0, 200)?;

        assert_eq!(claude.rows[0].session_id, long_id);
        assert_ne!(claude.rows[0].model, long_model);
        assert_eq!(
            claude.rows[0].model.chars().count(),
            SESSION_USAGE_MODEL_CHAR_LIMIT
        );
        assert!(claude.rows[0].model.starts_with("custom-model-"));
        assert!(claude.rows[0]
            .model
            .ends_with(&"x".repeat(SESSION_USAGE_MODEL_SUFFIX_CHARS)));
        assert_eq!(claude.rows[0].models, vec![claude.rows[0].model.clone()]);
        assert_eq!(claude.rows[0].model_count, 1);
        assert_eq!(codex.total_sessions, 1);
        assert_eq!(
            codex.rows[0].session_id,
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
        );
        assert_eq!(codex.rows[0].request_count, 2);
        Ok(())
    }

    #[test]
    fn serializes_cost_with_usage_dto_precision() {
        let row = SessionUsageRow {
            model: "provider/model-a".to_string(),
            models: vec![
                "provider/model-a".to_string(),
                "provider/model-b-20260726".to_string(),
            ],
            model_count: 2,
            total_cost_usd: 0.1 + 0.2,
            ..SessionUsageRow::default()
        };

        let json = serde_json::to_value(row).expect("serialize session usage row");

        assert_eq!(json["totalCostUsd"], "0.300000");
        assert_eq!(
            json["models"],
            serde_json::json!(["provider/model-a", "provider/model-b-20260726"])
        );
    }

    #[test]
    fn compact_session_ids_keep_both_ends() {
        let mut row = SessionUsageRow {
            session_id: "abcdefghijklAAA".to_string(),
            ..SessionUsageRow::default()
        };
        assert_eq!(row.compact_session_id(), "abcdefg…lAAA");

        row.session_id = "abcdefghijklBBB".to_string();
        assert_eq!(row.compact_session_id(), "abcdefg…lBBB");

        row.session_id = "short-id".to_string();
        assert_eq!(row.compact_session_id(), "short-id");

        row.model = "provider/model-with-shared-prefix-unique-tail".to_string();
        row.model_count = 2;
        assert_eq!(row.display_model_label(24), "provider/mode…ue-tail +1");
    }

    #[test]
    fn rejects_unsupported_apps_and_reversed_ranges() {
        let conn = setup_conn();

        assert!(query_session_usage(&conn, &AppType::Gemini, 0, 1).is_err());
        assert!(query_session_usage(&conn, &AppType::Claude, 2, 1).is_err());
    }
}
