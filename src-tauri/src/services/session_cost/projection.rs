use std::collections::{HashMap, HashSet};

use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection};

use crate::error::AppError;
use crate::services::sql_helpers::fresh_input_sql;
use crate::services::usage_stats::effective_usage_log_filter;
use crate::session_manager::SessionUsageSummary;

use super::{QueryControl, SessionCostIdentity};

#[derive(Debug)]
struct Aggregate {
    provider_id: String,
    logical_session_id: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    total_cost: f64,
    has_token_row: bool,
    has_unpriced_row: bool,
}

struct AggregateQuery {
    sql: String,
    values: Vec<Value>,
}

pub(crate) fn project_main_connection(
    conn: &Connection,
    identities: &[SessionCostIdentity],
    control: &QueryControl,
) -> Result<HashMap<SessionCostIdentity, SessionUsageSummary>, AppError> {
    if identities.is_empty() || control.is_cancelled() {
        return Ok(HashMap::new());
    }
    let identities = super::unambiguous_identities(identities);
    if identities.is_empty() {
        return Ok(HashMap::new());
    }

    let tx = conn
        .unchecked_transaction()
        .map_err(|error| AppError::Database(format!("begin session cost snapshot: {error}")))?;
    let aggregates = query_aggregates(&tx, &identities, control)?;
    if control.is_cancelled() {
        return Err(AppError::Message(
            "session cost projection cancelled".to_string(),
        ));
    }

    let mut by_logical = HashMap::new();
    for aggregate in aggregates {
        by_logical.insert(
            (
                aggregate.provider_id.clone(),
                aggregate.logical_session_id.clone(),
            ),
            aggregate,
        );
    }

    let mut overlays = HashMap::new();
    for identity in &identities {
        let key = (identity.provider_id.clone(), identity.session_id.clone());
        let Some(aggregate) = by_logical.get(&key) else {
            continue;
        };
        if !aggregate.has_token_row {
            continue;
        }
        let estimated_cost_usd = (!aggregate.has_unpriced_row)
            .then_some(aggregate.total_cost)
            .filter(|value| value.is_finite() && *value >= 0.0);
        overlays.insert(
            identity.clone(),
            SessionUsageSummary {
                input_tokens: aggregate.input_tokens,
                output_tokens: aggregate.output_tokens,
                cache_read_tokens: aggregate.cache_read_tokens,
                cache_creation_tokens: aggregate.cache_creation_tokens,
                estimated_cost_usd,
            },
        );
    }

    tx.commit()
        .map_err(|error| AppError::Database(format!("end session cost snapshot: {error}")))?;
    Ok(overlays)
}

fn query_aggregates(
    conn: &Connection,
    identities: &[SessionCostIdentity],
    control: &QueryControl,
) -> Result<Vec<Aggregate>, AppError> {
    let query = build_aggregate_query(identities);
    if query.values.is_empty() {
        return Ok(Vec::new());
    }

    if control.is_cancelled() {
        return Err(AppError::Message(
            "session cost projection cancelled".to_string(),
        ));
    }
    let mut statement = conn.prepare(&query.sql).map_err(AppError::from)?;
    let rows = statement
        .query_map(params_from_iter(query.values), |row| {
            Ok(Aggregate {
                provider_id: row.get(0)?,
                logical_session_id: row.get(1)?,
                input_tokens: nonnegative(row.get(2)?),
                output_tokens: nonnegative(row.get(3)?),
                cache_read_tokens: nonnegative(row.get(4)?),
                cache_creation_tokens: nonnegative(row.get(5)?),
                total_cost: row.get(6)?,
                has_token_row: row.get::<_, i64>(7)? != 0,
                has_unpriced_row: row.get::<_, i64>(8)? != 0,
            })
        })
        .map_err(AppError::from)?;

    let mut aggregates = Vec::new();
    for row in rows {
        if control.is_cancelled() {
            return Err(AppError::Message(
                "session cost projection cancelled".to_string(),
            ));
        }
        aggregates.push(row.map_err(AppError::from)?);
    }
    Ok(aggregates)
}

fn build_aggregate_query(identities: &[SessionCostIdentity]) -> AggregateQuery {
    let mut wanted = Vec::<(String, String, String)>::new();
    let mut seen = HashSet::new();
    for identity in identities {
        let provider = identity.provider_id.as_str();
        if !matches!(provider, "claude" | "codex" | "gemini" | "opencode") {
            continue;
        }
        let direct = (
            provider.to_string(),
            identity.session_id.clone(),
            identity.session_id.clone(),
        );
        if seen.insert(direct.clone()) {
            wanted.push(direct);
        }
        if provider == "codex" {
            let alias = (
                provider.to_string(),
                format!("codex_{}", identity.session_id),
                identity.session_id.clone(),
            );
            if seen.insert(alias.clone()) {
                wanted.push(alias);
            }
        }
    }
    if wanted.is_empty() {
        return AggregateQuery {
            sql: String::new(),
            values: Vec::new(),
        };
    }

    let values_sql = std::iter::repeat_n("(?, ?, ?)", wanted.len())
        .collect::<Vec<_>>()
        .join(", ");
    let fresh_input = fresh_input_sql("l");
    let effective_filter = effective_usage_log_filter("l");
    let row_tokens = format!(
        "({fresh_input} + l.output_tokens + l.cache_read_tokens + l.cache_creation_tokens)"
    );
    let numeric_cost = "CAST(l.total_cost_usd AS REAL)";
    let decimal_cost = canonical_decimal_text_sql("l.total_cost_usd");
    // `pricing_model` is write-time evidence, not a model name to resolve again:
    // NULL = legacy row, '' = explicitly unpriced, non-empty = successfully priced.
    // A legacy positive amount remains useful; a legacy zero cannot distinguish a
    // genuinely free request from a missing price and therefore fails closed.
    let trusted_cost = format!(
        "COALESCE(
            ({decimal_cost})
            AND {numeric_cost} >= 0
            AND (
                TRIM(l.pricing_model) <> ''
                OR (l.pricing_model IS NULL AND {numeric_cost} > 0)
            ),
            0
        )"
    );
    let unpriced = format!("{row_tokens} > 0 AND NOT ({trusted_cost})");
    let sql = format!(
        "WITH wanted(provider_id, lookup_session_id, logical_session_id) AS (
             VALUES {values_sql}
         )
         SELECT
             w.provider_id,
             w.logical_session_id,
             COALESCE(SUM({fresh_input}), 0),
             COALESCE(SUM(l.output_tokens), 0),
             COALESCE(SUM(l.cache_read_tokens), 0),
             COALESCE(SUM(l.cache_creation_tokens), 0),
             COALESCE(SUM(
                 CASE WHEN {row_tokens} > 0
                      THEN {numeric_cost} ELSE 0.0 END
             ), 0.0),
             MAX(CASE WHEN {row_tokens} > 0 THEN 1 ELSE 0 END),
             MAX(CASE WHEN {unpriced} THEN 1 ELSE 0 END)
         FROM wanted w
         JOIN proxy_request_logs l INDEXED BY idx_request_logs_session
           ON l.session_id = w.lookup_session_id
          AND l.app_type = w.provider_id
         WHERE {effective_filter}
         GROUP BY w.provider_id, w.logical_session_id"
    );

    let mut values = Vec::with_capacity(wanted.len() * 3);
    for (provider, lookup, logical) in wanted {
        values.push(Value::Text(provider));
        values.push(Value::Text(lookup));
        values.push(Value::Text(logical));
    }

    AggregateQuery { sql, values }
}

/// Accept only the simple decimal grammar emitted by CC Switch writers.
///
/// SQLite's numeric CAST turns malformed text such as `not-a-number` into
/// zero. That is convenient for reports but cannot prove a trustworthy
/// session estimate. Scientific notation is intentionally excluded because
/// the Decimal-based usage writers never emit it; unfamiliar historical text
/// fails closed instead of being partially parsed.
fn canonical_decimal_text_sql(expression: &str) -> String {
    let trimmed = format!("TRIM({expression})");
    let magnitude = format!(
        "(CASE WHEN SUBSTR({trimmed}, 1, 1) = '-'
               THEN SUBSTR({trimmed}, 2)
               ELSE {trimmed} END)"
    );
    format!(
        "{trimmed} <> ''
         AND {magnitude} <> ''
         AND {magnitude} NOT GLOB '*[^0-9.]*'
         AND (LENGTH({magnitude}) - LENGTH(REPLACE({magnitude}, '.', ''))) <= 1
         AND REPLACE({magnitude}, '.', '') <> ''"
    )
}

#[cfg(test)]
pub(super) fn explain_main_query_plan(
    conn: &Connection,
    identities: &[SessionCostIdentity],
) -> Result<Vec<String>, AppError> {
    let query = build_aggregate_query(identities);
    if query.values.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {}", query.sql))
        .map_err(AppError::from)?;
    let rows = statement
        .query_map(params_from_iter(query.values), |row| {
            row.get::<_, String>(3)
        })
        .map_err(AppError::from)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

fn nonnegative(value: i64) -> u64 {
    value.max(0) as u64
}
