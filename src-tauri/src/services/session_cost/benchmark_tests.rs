use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusqlite::{params_from_iter, Connection, OpenFlags};

use crate::database::database_path;
use crate::services::usage_stats::effective_usage_log_filter;

use super::{project_page, projection, QueryControl, SessionCostIdentity};

#[test]
#[ignore = "requires CC_SWITCH_SESSION_COST_BENCH_DB pointing at a read-only sandbox copy"]
#[serial_test::serial(home_settings)]
fn top_100_projection_on_explicit_database_copy() {
    let supplied = PathBuf::from(
        std::env::var_os("CC_SWITCH_SESSION_COST_BENCH_DB")
            .expect("set CC_SWITCH_SESSION_COST_BENCH_DB to a sandbox database copy"),
    );
    let configured = database_path().expect("resolve configured database");
    assert_eq!(
        supplied
            .canonicalize()
            .expect("canonicalize supplied benchmark database"),
        configured
            .canonicalize()
            .expect("canonicalize configured benchmark database"),
        "benchmark projection must read the explicitly supplied sandbox copy"
    );

    let conn = Connection::open_with_flags(
        &supplied,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .expect("open benchmark database read-only");
    conn.busy_timeout(Duration::from_millis(250))
        .expect("set benchmark busy timeout");
    conn.pragma_update(None, "query_only", true)
        .expect("enforce query-only benchmark connection");

    let identities = latest_identities(&conn);
    assert_eq!(
        identities.len(),
        100,
        "benchmark database must contain at least 100 attributable sessions"
    );
    let matched_rows = matched_detail_rows(&conn, &identities);
    let plan =
        projection::explain_main_query_plan(&conn, &identities).expect("explain projection query");
    assert!(
        plan.iter()
            .any(|line| line.contains("idx_request_logs_session")),
        "top-100 query must probe idx_request_logs_session:\n{}",
        plan.join("\n")
    );
    drop(conn);

    let mut samples = Vec::with_capacity(10);
    let mut cold_ms = 0.0;
    for iteration in 0..12 {
        let seq = iteration + 1;
        let control = QueryControl {
            active_cost_seq: Arc::new(AtomicU64::new(seq)),
            cost_seq: seq,
            deadline: Instant::now() + Duration::from_secs(2),
        };
        let started = Instant::now();
        let overlays = project_page(&identities, &control);
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "top-100 projection exceeded its hard deadline: {elapsed:?}"
        );
        if iteration == 0 {
            cold_ms = elapsed.as_secs_f64() * 1_000.0;
        } else if iteration >= 2 {
            samples.push(elapsed.as_secs_f64() * 1_000.0);
        }
        assert!(
            !overlays.is_empty(),
            "real-data benchmark should project at least one visible subtotal"
        );
    }
    samples.sort_by(f64::total_cmp);
    let p50 = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    eprintln!(
        "SESSION_COST_TOP_100 rows={matched_rows} cold_ms={cold_ms:.3} p50_ms={p50:.3} p95_ms={p95:.3} samples={}",
        samples.len()
    );
}

fn latest_identities(conn: &Connection) -> Vec<SessionCostIdentity> {
    let mut statement = conn
        .prepare(
            "SELECT app_type, session_id
             FROM proxy_request_logs
             WHERE app_type IN ('claude', 'codex', 'gemini', 'opencode')
               AND session_id IS NOT NULL
               AND TRIM(session_id) <> ''
             GROUP BY app_type, session_id
             ORDER BY MAX(created_at) DESC
             LIMIT 500",
        )
        .expect("prepare latest session query");
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("query latest sessions");

    let mut seen = HashSet::new();
    let mut identities = Vec::with_capacity(100);
    for row in rows {
        let (provider_id, stored_id) = row.expect("read latest session");
        let session_id = if provider_id == "codex" {
            stored_id
                .strip_prefix("codex_")
                .unwrap_or(&stored_id)
                .to_string()
        } else {
            stored_id
        };
        if !seen.insert((provider_id.clone(), session_id.clone())) {
            continue;
        }
        identities.push(SessionCostIdentity {
            provider_id,
            session_id,
            source_path: None,
        });
        if identities.len() == 100 {
            break;
        }
    }
    identities
}

fn matched_detail_rows(conn: &Connection, identities: &[SessionCostIdentity]) -> i64 {
    let mut wanted = Vec::with_capacity(identities.len() * 2);
    let mut seen = HashSet::new();
    for identity in identities {
        let direct = (identity.provider_id.clone(), identity.session_id.clone());
        if seen.insert(direct.clone()) {
            wanted.push(direct);
        }
        if identity.provider_id == "codex" {
            let alias = (
                "codex".to_string(),
                format!("codex_{}", identity.session_id),
            );
            if seen.insert(alias.clone()) {
                wanted.push(alias);
            }
        }
    }
    let values_sql = std::iter::repeat_n("(?, ?)", wanted.len())
        .collect::<Vec<_>>()
        .join(", ");
    let effective_filter = effective_usage_log_filter("l");
    let sql = format!(
        "WITH wanted(app_type, session_id) AS (VALUES {values_sql})
         SELECT COUNT(*)
         FROM wanted w
         JOIN proxy_request_logs l INDEXED BY idx_request_logs_session
           ON l.session_id = w.session_id
          AND l.app_type = w.app_type
         WHERE {effective_filter}"
    );
    let values = wanted
        .into_iter()
        .flat_map(|(provider, session)| [provider, session])
        .collect::<Vec<_>>();
    conn.query_row(&sql, params_from_iter(values), |row| row.get(0))
        .expect("count matched detail rows")
}

fn percentile(samples: &[f64], quantile: f64) -> f64 {
    let index = ((samples.len() as f64 * quantile).ceil() as usize)
        .saturating_sub(1)
        .min(samples.len().saturating_sub(1));
    samples[index]
}
