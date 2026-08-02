use std::collections::{HashMap, HashSet};
use std::time::Duration;

use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection, OpenFlags};

use crate::error::AppError;
use crate::session_manager::SessionUsageSummary;

use super::{QueryControl, SessionCostIdentity};

pub(super) fn project(
    identities: &[SessionCostIdentity],
    control: &QueryControl,
) -> Result<HashMap<SessionCostIdentity, SessionUsageSummary>, AppError> {
    let path = crate::hermes_config::get_hermes_dir().join("state.db");
    if !path.exists() || control.is_cancelled() {
        return Ok(HashMap::new());
    }
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(AppError::from)?;
    conn.busy_timeout(Duration::from_millis(250))
        .map_err(AppError::from)?;
    conn.pragma_update(None, "query_only", true)
        .map_err(AppError::from)?;
    control.install_progress_handler(&conn);
    let result = project_connection(&conn, identities, control);
    QueryControl::clear_progress_handler(&conn);
    result
}

fn project_connection(
    conn: &Connection,
    identities: &[SessionCostIdentity],
    control: &QueryControl,
) -> Result<HashMap<SessionCostIdentity, SessionUsageSummary>, AppError> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| AppError::Database(format!("begin Hermes cost snapshot: {error}")))?;
    let overlays = project_snapshot(&tx, identities, control)?;
    tx.commit()
        .map_err(|error| AppError::Database(format!("end Hermes cost snapshot: {error}")))?;
    Ok(overlays)
}

fn project_snapshot(
    conn: &Connection,
    identities: &[SessionCostIdentity],
    control: &QueryControl,
) -> Result<HashMap<SessionCostIdentity, SessionUsageSummary>, AppError> {
    if !table_exists(conn, "sessions")? {
        return Ok(HashMap::new());
    }
    let columns = table_columns(conn, "sessions")?;
    if [
        "id",
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
    ]
    .iter()
    .any(|column| !columns.contains(*column))
    {
        return Ok(HashMap::new());
    }

    let wanted = identities
        .iter()
        .map(|identity| identity.session_id.clone())
        .collect::<HashSet<_>>();
    let by_session = query_sessions(conn, &columns, &wanted, control)?
        .into_iter()
        .collect::<HashMap<_, _>>();

    let mut overlays = HashMap::new();
    for identity in identities {
        let Some(usage) = by_session.get(&identity.session_id).copied() else {
            continue;
        };
        if usage.is_empty() {
            continue;
        }
        overlays.insert(identity.clone(), usage.into_summary());
    }
    Ok(overlays)
}

fn query_sessions(
    conn: &Connection,
    columns: &HashSet<String>,
    wanted: &HashSet<String>,
    control: &QueryControl,
) -> Result<Vec<(String, UsageBuckets)>, AppError> {
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let cost = if columns.contains("estimated_cost_usd") {
        "source.estimated_cost_usd"
    } else {
        "NULL"
    };
    let values = std::iter::repeat_n("(?)", wanted.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "WITH wanted(session_id) AS (VALUES {values})
         SELECT wanted.session_id,
                source.input_tokens,
                source.output_tokens,
                source.cache_read_tokens,
                source.cache_write_tokens,
                {cost} AS estimated_cost_usd
         FROM wanted
         JOIN sessions source ON source.id = wanted.session_id",
    );
    let bindings = wanted.iter().cloned().map(Value::Text).collect::<Vec<_>>();
    let mut statement = conn.prepare(&sql).map_err(AppError::from)?;
    let rows = statement
        .query_map(params_from_iter(bindings), |row| {
            let session_id = row.get::<_, String>(0)?;
            let Some(input_tokens) = valid_tokens(row.get(1)?) else {
                return Ok(None);
            };
            let Some(output_tokens) = valid_tokens(row.get(2)?) else {
                return Ok(None);
            };
            let Some(cache_read_tokens) = valid_tokens(row.get(3)?) else {
                return Ok(None);
            };
            let Some(cache_creation_tokens) = valid_tokens(row.get(4)?) else {
                return Ok(None);
            };
            Ok(Some((
                session_id,
                UsageBuckets {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_creation_tokens,
                    estimated_cost_usd: valid_cost(row.get(5)?),
                },
            )))
        })
        .map_err(AppError::from)?;
    let mut result = Vec::new();
    for row in rows {
        if control.is_cancelled() {
            return Err(AppError::Message(
                "Hermes session cost projection cancelled".to_string(),
            ));
        }
        if let Some(row) = row.map_err(AppError::from)? {
            result.push(row);
        }
    }
    Ok(result)
}

#[derive(Debug, Clone, Copy, Default)]
struct UsageBuckets {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    estimated_cost_usd: Option<f64>,
}

impl UsageBuckets {
    fn is_empty(self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.cache_read_tokens == 0
            && self.cache_creation_tokens == 0
    }

    fn into_summary(self) -> SessionUsageSummary {
        SessionUsageSummary {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_creation_tokens: self.cache_creation_tokens,
            estimated_cost_usd: self.estimated_cost_usd,
        }
    }
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1
         )",
        [table],
        |row| row.get(0),
    )
    .map_err(AppError::from)
}

fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>, AppError> {
    let mut statement = conn
        .prepare("SELECT name FROM pragma_table_info(?1)")
        .map_err(AppError::from)?;
    let rows = statement
        .query_map([table], |row| row.get::<_, String>(0))
        .map_err(AppError::from)?;
    rows.collect::<Result<HashSet<_>, _>>()
        .map_err(AppError::from)
}

fn valid_tokens(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

fn valid_cost(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value >= 0.0)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use rusqlite::Connection;

    use crate::services::session_cost::{QueryControl, SessionCostIdentity};

    use super::project_connection;

    fn control() -> QueryControl {
        QueryControl {
            active_cost_seq: Arc::new(AtomicU64::new(1)),
            cost_seq: 1,
            deadline: Instant::now() + Duration::from_secs(2),
        }
    }

    fn target(session_id: &str) -> SessionCostIdentity {
        SessionCostIdentity {
            provider_id: "hermes".to_string(),
            session_id: session_id.to_string(),
            source_path: Some(format!("state.db#{session_id}")),
        }
    }

    #[test]
    fn wanted_sessions_are_projected_as_estimates_from_hermes_state() {
        let conn = Connection::open_in_memory().expect("Hermes fixture");
        conn.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT NOT NULL,
                 input_tokens INTEGER NOT NULL,
                 output_tokens INTEGER NOT NULL,
                 cache_read_tokens INTEGER NOT NULL,
                 cache_write_tokens INTEGER NOT NULL,
                 estimated_cost_usd REAL NOT NULL
             );
             INSERT INTO sessions VALUES
                 ('wanted', 11, 22, 33, 44, 0.75),
                 ('unrelated', 999, 999, 999, 999, 99.0);",
        )
        .expect("seed Hermes fixture");

        let wanted = target("wanted");
        let overlays = project_connection(&conn, std::slice::from_ref(&wanted), &control())
            .expect("project Hermes state");
        assert_eq!(overlays.len(), 1);
        let usage = overlays.get(&wanted).expect("wanted Hermes overlay");
        assert_eq!(
            (
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_tokens,
                usage.cache_creation_tokens,
            ),
            (11, 22, 33, 44)
        );
        assert_eq!(usage.estimated_cost_usd, Some(0.75));
    }

    #[test]
    fn token_data_without_a_cost_column_keeps_cost_unavailable() {
        let conn = Connection::open_in_memory().expect("Hermes fixture");
        conn.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT NOT NULL,
                 input_tokens INTEGER NOT NULL,
                 output_tokens INTEGER NOT NULL,
                 cache_read_tokens INTEGER NOT NULL,
                 cache_write_tokens INTEGER NOT NULL
             );
             INSERT INTO sessions VALUES ('wanted', 10, 5, 2, 1);",
        )
        .expect("seed Hermes fixture");

        let wanted = target("wanted");
        let overlays = project_connection(&conn, std::slice::from_ref(&wanted), &control())
            .expect("project Hermes state");
        let usage = overlays.get(&wanted).expect("wanted Hermes overlay");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.estimated_cost_usd, None);
    }

    #[test]
    fn missing_any_token_column_fails_closed_instead_of_inventing_zero() {
        let conn = Connection::open_in_memory().expect("Hermes fixture");
        conn.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT NOT NULL,
                 input_tokens INTEGER NOT NULL,
                 output_tokens INTEGER NOT NULL,
                 cache_read_tokens INTEGER NOT NULL,
                 estimated_cost_usd REAL
             );
             INSERT INTO sessions VALUES ('wanted', 10, 5, 2, 0.75);",
        )
        .expect("seed incomplete Hermes schema");

        let wanted = target("wanted");
        let overlays = project_connection(&conn, std::slice::from_ref(&wanted), &control())
            .expect("project incomplete Hermes state");

        assert!(
            overlays.is_empty(),
            "a missing cache_write_tokens column is unknown, not a trustworthy zero"
        );
    }

    #[test]
    fn null_or_negative_token_values_fail_closed() {
        let conn = Connection::open_in_memory().expect("Hermes fixture");
        conn.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT NOT NULL,
                 input_tokens INTEGER,
                 output_tokens INTEGER,
                 cache_read_tokens INTEGER,
                 cache_write_tokens INTEGER,
                 estimated_cost_usd REAL
             );
             INSERT INTO sessions VALUES
                 ('null-token', 10, 5, NULL, 1, 0.75),
                 ('negative-token', 10, 5, -1, 1, 0.75);",
        )
        .expect("seed invalid Hermes rows");

        let targets = [target("null-token"), target("negative-token")];
        let overlays =
            project_connection(&conn, &targets, &control()).expect("project invalid Hermes rows");

        assert!(
            overlays.is_empty(),
            "invalid external token values must not be normalized into believable usage"
        );
    }

    #[test]
    fn actual_cost_column_is_not_relabelled_as_an_estimate() {
        let conn = Connection::open_in_memory().expect("Hermes fixture");
        conn.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT NOT NULL,
                 input_tokens INTEGER NOT NULL,
                 output_tokens INTEGER NOT NULL,
                 cache_read_tokens INTEGER NOT NULL,
                 cache_write_tokens INTEGER NOT NULL,
                 actual_cost_usd REAL NOT NULL
             );
             INSERT INTO sessions VALUES ('wanted', 10, 5, 0, 0, 0.75);",
        )
        .expect("seed Hermes fixture");

        let wanted = target("wanted");
        let overlays = project_connection(&conn, std::slice::from_ref(&wanted), &control())
            .expect("project Hermes state");
        let usage = overlays.get(&wanted).expect("wanted Hermes overlay");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(
            usage.estimated_cost_usd, None,
            "only the source's estimated_cost_usd field may be displayed"
        );
    }

    #[test]
    fn null_estimated_cost_is_not_coalesced_into_a_zero_estimate() {
        let conn = Connection::open_in_memory().expect("Hermes fixture");
        conn.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT NOT NULL,
                 input_tokens INTEGER NOT NULL,
                 output_tokens INTEGER NOT NULL,
                 cache_read_tokens INTEGER NOT NULL,
                 cache_write_tokens INTEGER NOT NULL,
                 estimated_cost_usd REAL
             );
             INSERT INTO sessions VALUES ('wanted', 10, 5, 0, 0, NULL);",
        )
        .expect("seed Hermes fixture");

        let wanted = target("wanted");
        let overlays = project_connection(&conn, std::slice::from_ref(&wanted), &control())
            .expect("project Hermes state");
        let usage = overlays.get(&wanted).expect("wanted Hermes overlay");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(
            usage.estimated_cost_usd, None,
            "missing source estimates must render as unavailable, not $0"
        );
    }

    #[test]
    fn session_estimate_must_be_valid_when_tokens_are_present() {
        let conn = Connection::open_in_memory().expect("Hermes fixture");
        conn.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT NOT NULL,
                 input_tokens INTEGER NOT NULL,
                 output_tokens INTEGER NOT NULL,
                 cache_read_tokens INTEGER NOT NULL,
                 cache_write_tokens INTEGER NOT NULL,
                 estimated_cost_usd REAL
             );
             INSERT INTO sessions VALUES
                 ('missing', 12, 6, 0, 0, NULL),
                 ('priced', 10, 5, 0, 0, 0.5),
                 ('negative', 12, 6, 0, 0, -0.1);",
        )
        .expect("seed Hermes fixture");

        let missing = target("missing");
        let priced = target("priced");
        let negative = target("negative");
        let overlays = project_connection(
            &conn,
            &[missing.clone(), priced.clone(), negative.clone()],
            &control(),
        )
        .expect("project Hermes state");

        let missing_usage = overlays.get(&missing).expect("missing overlay");
        assert_eq!(
            (missing_usage.input_tokens, missing_usage.output_tokens),
            (12, 6)
        );
        assert_eq!(missing_usage.estimated_cost_usd, None);

        let priced_usage = overlays.get(&priced).expect("priced overlay");
        assert_eq!(priced_usage.estimated_cost_usd, Some(0.5));

        let negative_usage = overlays.get(&negative).expect("negative overlay");
        assert_eq!(negative_usage.estimated_cost_usd, None);
    }

    #[test]
    fn sessions_table_is_the_only_authoritative_session_total() {
        let conn = Connection::open_in_memory().expect("Hermes fixture");
        conn.execute_batch(
            "CREATE TABLE session_model_usage (
                 session_id TEXT NOT NULL,
                 input_tokens INTEGER NOT NULL,
                 output_tokens INTEGER NOT NULL,
                 cache_read_tokens INTEGER NOT NULL,
                 cache_write_tokens INTEGER NOT NULL,
                 estimated_cost_usd REAL
             );
             CREATE TABLE sessions (
                 id TEXT NOT NULL,
                 input_tokens INTEGER NOT NULL,
                 output_tokens INTEGER NOT NULL,
                 cache_read_tokens INTEGER NOT NULL,
                 cache_write_tokens INTEGER NOT NULL,
                 estimated_cost_usd REAL
             );
             INSERT INTO session_model_usage VALUES
                 ('wanted', 100, 20, 30, 40, 0.75),
                 ('attribution-only', 90, 9, 8, 7, 0.65);
             INSERT INTO sessions VALUES
                 ('wanted', 10, 2, 3, 4, 0.5);",
        )
        .expect("seed dual-table Hermes fixture");

        let wanted = target("wanted");
        let attribution_only = target("attribution-only");
        let overlays = project_connection(
            &conn,
            &[wanted.clone(), attribution_only.clone()],
            &control(),
        )
        .expect("project dual-table Hermes state");

        assert_eq!(
            overlays.get(&wanted),
            Some(&crate::session_manager::SessionUsageSummary {
                input_tokens: 10,
                output_tokens: 2,
                cache_read_tokens: 3,
                cache_creation_tokens: 4,
                estimated_cost_usd: Some(0.5),
            }),
            "tokens and cost must come from the complete sessions row"
        );
        assert_eq!(
            overlays.get(&attribution_only),
            None,
            "per-model attribution rows are not session-level totals"
        );
    }

    #[test]
    fn cost_only_rows_have_no_displayable_usage_without_token_bearing_rows() {
        let conn = Connection::open_in_memory().expect("Hermes fixture");
        conn.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT NOT NULL,
                 estimated_cost_usd REAL
             );
             INSERT INTO sessions VALUES
                 ('positive', 0.75),
                 ('zero', 0.0),
                 ('missing', NULL);",
        )
        .expect("seed Hermes fixture");

        let targets = [target("positive"), target("zero"), target("missing")];
        let overlays =
            project_connection(&conn, &targets, &control()).expect("project Hermes state");

        assert!(
            overlays.is_empty(),
            "Hermes estimates require at least one token-bearing attributable row"
        );
    }
}
