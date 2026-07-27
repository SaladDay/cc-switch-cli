//! Usage rollup DAO
//!
//! Aggregates proxy_request_logs into daily rollups and prunes old detail rows.

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::services::session_usage::{acquire_session_sync_guard, SessionSyncGuard};
use crate::services::sql_helpers::{fresh_input_sql, INPUT_TOKEN_SEMANTICS_FRESH};
use crate::services::usage_stats::{
    effective_usage_log_filter, identified_matched_request_pairs_sql,
};
use chrono::{Days, Local};

/// Compute the rollup/prune cutoff aligned to a local-day boundary.
///
/// Anything strictly older than the returned timestamp will be aggregated into
/// `usage_daily_rollups` and deleted from `proxy_request_logs`. Aligning to the
/// next local day after the calendar retention window guarantees that the youngest
/// rollup row always represents a *complete* local day. Without this alignment
/// the cutoff falls mid-day, leaving the day half-rolled-up and half-pruned —
/// which would silently under-count any range query that touches that day
/// after `compute_rollup_date_bounds` trims partial-coverage rollup days.
fn compute_local_midnight_cutoff(
    now: chrono::DateTime<Local>,
    retain_days: i64,
) -> Result<i64, AppError> {
    let target_day = retention_target_day(now.date_naive(), retain_days)?;
    let next_day = target_day
        .succ_opt()
        .ok_or_else(|| AppError::Database("rollup cutoff next-day overflow".to_string()))?;
    crate::services::session_usage_query::local_day_start_timestamp(next_day)
        .ok_or_else(|| AppError::Database("rollup cutoff day has no valid local time".to_string()))
}

fn retention_target_day(
    today: chrono::NaiveDate,
    retain_days: i64,
) -> Result<chrono::NaiveDate, AppError> {
    let retain_days = u64::try_from(retain_days)
        .map_err(|_| AppError::Database("rollup retention must be non-negative".to_string()))?;
    today
        .checked_sub_days(Days::new(retain_days))
        .ok_or_else(|| AppError::Database("rollup cutoff overflow".to_string()))
}

impl Database {
    /// Aggregate proxy_request_logs older than `retain_days` into usage_daily_rollups,
    /// then delete the aggregated detail rows.
    /// Returns the number of deleted detail rows.
    pub fn rollup_and_prune(&self, retain_days: i64) -> Result<u64, AppError> {
        let guard = acquire_session_sync_guard(self)?;
        self.rollup_and_prune_with_session_guard(retain_days, &guard)
    }

    /// Roll up usage while the caller already owns the session-usage lock.
    ///
    /// Migration holds this lock across schema reset and startup maintenance,
    /// so it must reuse the guard instead of recursively acquiring it.
    pub(crate) fn rollup_and_prune_with_session_guard(
        &self,
        retain_days: i64,
        _guard: &SessionSyncGuard,
    ) -> Result<u64, AppError> {
        let cutoff = compute_local_midnight_cutoff(Local::now(), retain_days)?;
        let conn = lock_conn!(self.conn);

        // Check if there are any rows to process
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE created_at < ?1",
                [cutoff],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        if count == 0 {
            return Ok(0);
        }

        // Use a savepoint for atomicity
        conn.execute("SAVEPOINT rollup_prune;", [])
            .map_err(|e| AppError::Database(e.to_string()))?;

        let result = Self::do_rollup_and_prune(&conn, cutoff);

        match result {
            Ok(deleted) => {
                conn.execute("RELEASE rollup_prune;", [])
                    .map_err(|e| AppError::Database(e.to_string()))?;
                if deleted > 0 {
                    log::info!(
                        "Rolled up and pruned {deleted} proxy_request_logs (retain={retain_days}d)"
                    );
                    // 归档触发了表结构变化，前端 30 天前的统计可能跟着变，
                    // 通知一次让 UsageDashboard 重拉数据
                    crate::usage_events::notify_log_recorded();
                }
                Ok(deleted)
            }
            Err(e) => {
                conn.execute("ROLLBACK TO rollup_prune;", []).ok();
                conn.execute("RELEASE rollup_prune;", []).ok();
                Err(e)
            }
        }
    }

    fn do_rollup_and_prune(conn: &rusqlite::Connection, cutoff: i64) -> Result<u64, AppError> {
        // Aggregate old logs, merging with any pre-existing rollup rows via LEFT JOIN.
        let effective_filter = effective_usage_log_filter("l");
        let fresh_detail_input = fresh_input_sql("l");
        let fresh_old_input = fresh_input_sql("old");
        let aggregation_sql = format!(
            "INSERT OR REPLACE INTO usage_daily_rollups
                (date, app_type, provider_id, model, request_model, pricing_model,
                 request_count, success_count,
                 input_tokens, output_tokens,
                 cache_read_tokens, cache_creation_tokens,
                 input_token_semantics, total_cost_usd, avg_latency_ms)
            SELECT
                d, a, p, m, rm, pm,
                COALESCE(old.request_count, 0) + new_req,
                COALESCE(old.success_count, 0) + new_succ,
                COALESCE({fresh_old_input}, 0) + new_in,
                COALESCE(old.output_tokens, 0) + new_out,
                COALESCE(old.cache_read_tokens, 0) + new_cr,
                COALESCE(old.cache_creation_tokens, 0) + new_cc,
                {INPUT_TOKEN_SEMANTICS_FRESH},
                CAST(COALESCE(CAST(old.total_cost_usd AS REAL), 0) + new_cost AS TEXT),
                CASE WHEN COALESCE(old.request_count, 0) + new_req > 0
                    THEN (COALESCE(old.avg_latency_ms, 0) * COALESCE(old.request_count, 0)
                          + new_lat * new_req)
                         / (COALESCE(old.request_count, 0) + new_req)
                    ELSE 0 END
            FROM (
                SELECT
                    date(l.created_at, 'unixepoch', 'localtime') as d,
                    l.app_type as a, l.provider_id as p, l.model as m,
                    COALESCE(l.request_model, '') as rm,
                    COALESCE(l.pricing_model, '') as pm,
                    COUNT(*) as new_req,
                    SUM(CASE WHEN l.status_code >= 200 AND l.status_code < 300 THEN 1 ELSE 0 END) as new_succ,
                    COALESCE(SUM({fresh_detail_input}), 0) as new_in,
                    COALESCE(SUM(l.output_tokens), 0) as new_out,
                    COALESCE(SUM(l.cache_read_tokens), 0) as new_cr,
                    COALESCE(SUM(l.cache_creation_tokens), 0) as new_cc,
                    COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0) as new_cost,
                    COALESCE(AVG(l.latency_ms), 0) as new_lat
                FROM proxy_request_logs l
                WHERE l.created_at < ?1 AND {effective_filter}
                GROUP BY d, a, p, m, rm, pm
            ) agg
            LEFT JOIN usage_daily_rollups old
                ON old.date = agg.d AND old.app_type = agg.a
                AND old.provider_id = agg.p AND old.model = agg.m
                AND old.request_model = agg.rm AND old.pricing_model = agg.pm"
        );

        conn.execute(&aggregation_sql, [cutoff])
            .map_err(|e| AppError::Database(format!("Rollup aggregation failed: {e}")))?;

        // The effective filter above chooses proxy as the winner for each
        // identified proxy/import pair. If the cutoff separates the pair, the
        // winner is about to lose its request identity in the rollup while the
        // excluded import would remain in detail and become effective later.
        // Fold only those known cross-cutoff losers while both IDs are still
        // available. Production retains one extra day beyond the 30-day
        // Sessions window, so this cannot remove visible session detail.
        let matched_pairs = identified_matched_request_pairs_sql(None);
        let fold_cross_cutoff_imports_sql = format!(
            "DELETE FROM proxy_request_logs
             WHERE request_id IN (
                 SELECT matched_pairs.excluded_request_id
                 FROM ({matched_pairs}) matched_pairs
                 JOIN proxy_request_logs winner_proxy
                   ON winner_proxy.request_id = matched_pairs.proxy_request_id
                 JOIN proxy_request_logs excluded_import
                   ON excluded_import.request_id =
                      matched_pairs.excluded_request_id
                 WHERE winner_proxy.created_at < ?1
                   AND excluded_import.created_at >= ?1
             )"
        );
        let folded_imports = conn
            .execute(&fold_cross_cutoff_imports_sql, [cutoff])
            .map_err(|e| {
                AppError::Database(format!("Folding cross-cutoff usage matches failed: {e}"))
            })?;

        // INSERT uses the effective-log filter to exclude duplicate session rows.
        // DELETE intentionally prunes all old details so those duplicates are discarded.
        let pruned = conn
            .execute(
                "DELETE FROM proxy_request_logs WHERE created_at < ?1",
                [cutoff],
            )
            .map_err(|e| AppError::Database(format!("Pruning old logs failed: {e}")))?;

        Ok((folded_imports + pruned) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::{compute_local_midnight_cutoff, retention_target_day};
    use crate::database::Database;
    use crate::error::AppError;
    use crate::services::sql_helpers::INPUT_TOKEN_SEMANTICS_FRESH;
    use chrono::{Local, TimeZone};

    fn local_dt(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> chrono::DateTime<Local> {
        match Local.with_ymd_and_hms(year, month, day, hour, minute, second) {
            chrono::LocalResult::Single(dt) => dt,
            chrono::LocalResult::Ambiguous(earliest, _) => earliest,
            chrono::LocalResult::None => panic!("invalid local datetime in test fixture"),
        }
    }

    #[test]
    fn cutoff_is_aligned_to_local_midnight_after_target_day() -> Result<(), AppError> {
        // now = 2026-04-16 14:32:17 local; retain_days = 30
        // target day = 2026-03-17; cutoff should be 2026-03-18 00:00 local.
        let now = local_dt(2026, 4, 16, 14, 32, 17);
        let cutoff_ts = compute_local_midnight_cutoff(now, 30)?;
        let cutoff_dt = Local.timestamp_opt(cutoff_ts, 0).single().unwrap();
        let expected = local_dt(2026, 3, 18, 0, 0, 0);
        assert_eq!(cutoff_dt, expected);
        Ok(())
    }

    #[test]
    fn cutoff_at_local_midnight_now_still_lands_on_midnight() -> Result<(), AppError> {
        // If `now` is itself local midnight, the math should not introduce drift.
        let now = local_dt(2026, 4, 16, 0, 0, 0);
        let cutoff_ts = compute_local_midnight_cutoff(now, 7)?;
        let cutoff_dt = Local.timestamp_opt(cutoff_ts, 0).single().unwrap();
        // (2026-04-16 - 7d) = 2026-04-09; cutoff = 2026-04-10 00:00 local.
        let expected = local_dt(2026, 4, 10, 0, 0, 0);
        assert_eq!(cutoff_dt, expected);
        Ok(())
    }

    #[test]
    fn retention_target_uses_calendar_days() -> Result<(), AppError> {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        assert_eq!(
            retention_target_day(today, 31)?,
            chrono::NaiveDate::from_ymd_opt(2026, 10, 20).unwrap()
        );
        Ok(())
    }

    #[test]
    fn rollup_waits_for_the_session_sync_guard() -> Result<(), AppError> {
        use crate::services::session_usage::acquire_session_sync_guard;
        use std::sync::{mpsc, Arc};
        use std::time::Duration;

        let db = Arc::new(Database::memory()?);
        let old_ts = chrono::Utc::now().timestamp() - 45 * 86_400;
        {
            let conn = db.conn.lock().expect("lock database");
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model,
                    input_tokens, output_tokens, total_cost_usd,
                    latency_ms, status_code, created_at, data_source
                 ) VALUES (
                    'rollup-race-proxy', 'proxy-provider', 'codex', 'gpt',
                    10, 2, '0.01', 10, 200, ?1, 'proxy'
                 )",
                [old_ts],
            )
            .expect("seed old Codex proxy detail");
        }

        let guard = acquire_session_sync_guard(&db).expect("hold session safety window");
        db.ensure_codex_usage_rebuild_safe()
            .expect("rebuild should be safe before rollup");

        let contender = Arc::clone(&db);
        let (attempted_tx, attempted_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            attempted_tx.send(()).expect("signal rollup attempt");
            finished_tx
                .send(contender.rollup_and_prune(31))
                .expect("signal rollup completion");
        });
        attempted_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("rollup thread did not start");
        assert!(
            finished_rx
                .recv_timeout(Duration::from_millis(150))
                .is_err(),
            "rollup crossed the held rebuild safety window"
        );
        drop(guard);
        assert_eq!(
            finished_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("rollup did not resume")?,
            1
        );
        thread.join().expect("join rollup thread");

        let conn = crate::database::lock_conn!(db.conn);
        let counts: (i64, i64) = conn.query_row(
            "SELECT
                (SELECT COUNT(*) FROM proxy_request_logs
                 WHERE request_id = 'rollup-race-proxy'),
                (SELECT COUNT(*) FROM usage_daily_rollups
                 WHERE app_type = 'codex' AND provider_id = 'proxy-provider')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(counts, (0, 1));
        Ok(())
    }

    #[test]
    fn test_rollup_and_prune() -> Result<(), AppError> {
        let db = Database::memory()?;
        let now = chrono::Utc::now().timestamp();
        let old_ts = now - 40 * 86400; // 40 days ago
        let recent_ts = now - 5 * 86400; // 5 days ago

        {
            let conn = crate::database::lock_conn!(db.conn);
            for i in 0..5 {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model,
                        input_tokens, output_tokens, total_cost_usd,
                        latency_ms, status_code, created_at
                    ) VALUES (?1, 'p1', 'claude', 'claude-3', 100, 50, '0.01', 100, 200, ?2)",
                    rusqlite::params![format!("old-{i}"), old_ts + i as i64],
                )?;
            }
            for i in 0..3 {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model,
                        input_tokens, output_tokens, total_cost_usd,
                        latency_ms, status_code, created_at
                    ) VALUES (?1, 'p1', 'claude', 'claude-3', 200, 100, '0.02', 150, 200, ?2)",
                    rusqlite::params![format!("recent-{i}"), recent_ts + i as i64],
                )?;
            }
        }

        let deleted = db.rollup_and_prune(30)?;
        assert_eq!(deleted, 5);

        // Verify rollup data
        let conn = crate::database::lock_conn!(db.conn);
        let count: i64 = conn.query_row(
            "SELECT request_count FROM usage_daily_rollups WHERE app_type = 'claude'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(count, 5);

        // Verify recent logs untouched
        let remaining: i64 =
            conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
                row.get(0)
            })?;
        assert_eq!(remaining, 3);
        Ok(())
    }

    #[test]
    fn test_rollup_uses_effective_usage_logs() -> Result<(), AppError> {
        let db = Database::memory()?;
        let old_ts = compute_local_midnight_cutoff(Local::now(), 40)?
            - chrono::Duration::hours(12).num_seconds();

        {
            let conn = crate::database::lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    total_cost_usd, latency_ms, status_code, session_id, created_at, data_source
                ) VALUES (?1, 'openai', 'codex', 'gpt-5.4', 'gpt-5.4', 100, 20, 10, 0, '0.10', 100, 200, 'codex_thread-a', ?2, 'proxy')",
                rusqlite::params!["codex-proxy-old", old_ts],
            )?;
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    total_cost_usd, latency_ms, status_code, session_id, created_at, data_source
                ) VALUES (?1, '_codex_session', 'codex', 'gpt-5.4', 'gpt-5.4', 100, 20, 10, 0, '0.10', 0, 200, 'thread-a', ?2, 'codex_session')",
                rusqlite::params!["codex-session-old-dup", old_ts + 60],
            )?;
        }

        let deleted = db.rollup_and_prune(30)?;
        assert_eq!(deleted, 2);

        let conn = crate::database::lock_conn!(db.conn);
        let mut stmt = conn.prepare(
            "SELECT provider_id, request_count, input_tokens, output_tokens, cache_read_tokens
             FROM usage_daily_rollups WHERE app_type = 'codex'",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        assert_eq!(rows.len(), 1);
        let (provider_id, request_count, input_tokens, output_tokens, cache_read_tokens) = &rows[0];
        assert_eq!(provider_id, "openai");
        assert_eq!(*request_count, 1);
        assert_eq!(*input_tokens, 90, "rollup stores normalized fresh input");
        assert_eq!(*output_tokens, 20);
        assert_eq!(*cache_read_tokens, 10);

        let remaining: i64 =
            conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
                row.get(0)
            })?;
        assert_eq!(remaining, 0);

        Ok(())
    }

    #[test]
    fn test_rollup_folds_only_matched_import_across_cutoff() -> Result<(), AppError> {
        let db = Database::memory()?;
        let cutoff = compute_local_midnight_cutoff(Local::now(), 30)?;

        {
            let conn = crate::database::lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    total_cost_usd, latency_ms, status_code, session_id, created_at, data_source
                ) VALUES (
                    'straddle-proxy', 'openai', 'codex', 'openai/gpt-5.4',
                    'gpt-5.4', 100, 20, 10, 7, '0.10', 100, 200,
                    'codex_thread-straddle', ?1, 'proxy'
                )",
                [cutoff - 60],
            )?;
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    total_cost_usd, latency_ms, status_code, session_id, created_at, data_source
                ) VALUES (
                    'straddle-import', '_codex_session', 'codex', 'gpt-5.4',
                    'gpt-5.4', 100, 20, 10, 0, '0.10', 0, 200,
                    'thread-straddle', ?1, 'codex_session'
                )",
                [cutoff + 60],
            )?;
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    total_cost_usd, latency_ms, status_code, session_id, created_at, data_source
                ) VALUES (
                    'unmatched-import', '_codex_session', 'codex', 'gpt-5.4',
                    'gpt-5.4', 33, 4, 3, 0, '0.03', 0, 200,
                    'thread-straddle', ?1, 'codex_session'
                )",
                [cutoff + 120],
            )?;
        }

        assert_eq!(
            db.rollup_and_prune(30)?,
            2,
            "the old proxy and its retained duplicate are both removed"
        );

        {
            let conn = crate::database::lock_conn!(db.conn);
            let rolled_requests: i64 = conn.query_row(
                "SELECT COALESCE(SUM(request_count), 0)
                 FROM usage_daily_rollups
                 WHERE app_type = 'codex'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(
                rolled_requests, 1,
                "the matched proxy/import pair must enter rollup exactly once"
            );

            let remaining_ids = conn
                .prepare("SELECT request_id FROM proxy_request_logs ORDER BY request_id")?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(
                remaining_ids,
                vec!["unmatched-import"],
                "only the precisely matched retained import may be folded"
            );
        }

        let summary = db.get_usage_summary(None, None, Some("codex"))?;
        assert_eq!(
            summary.total_requests, 2,
            "one rolled match plus one legitimate unmatched import"
        );
        assert_eq!(summary.total_input_tokens, 120);
        assert_eq!(summary.total_output_tokens, 24);
        assert_eq!(summary.total_cache_read_tokens, 13);

        Ok(())
    }

    #[test]
    fn test_rollup_normalizes_total_cache_semantics_to_fresh() -> Result<(), AppError> {
        let db = Database::memory()?;
        let old_ts = chrono::Utc::now().timestamp() - 40 * 86400;

        {
            let conn = crate::database::lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    input_token_semantics, total_cost_usd,
                    latency_ms, status_code, created_at
                ) VALUES ('total-semantics-rollup', 'p1', 'codex', 'gpt-5.5',
                          100, 5, 10, 20, 1, '0.10', 100, 200, ?1)",
                [old_ts],
            )?;
        }

        assert_eq!(db.rollup_and_prune(30)?, 1);

        let conn = crate::database::lock_conn!(db.conn);
        let row: (i64, i64, i64, i64) = conn.query_row(
            "SELECT input_tokens, cache_read_tokens, cache_creation_tokens,
                    input_token_semantics
             FROM usage_daily_rollups WHERE model = 'gpt-5.5'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        assert_eq!(row, (70, 10, 20, INPUT_TOKEN_SEMANTICS_FRESH));

        Ok(())
    }

    #[test]
    fn test_rollup_noop_when_no_old_data() -> Result<(), AppError> {
        let db = Database::memory()?;
        assert_eq!(db.rollup_and_prune(30)?, 0);
        Ok(())
    }

    #[test]
    fn test_rollup_preserves_request_and_pricing_model_dimensions() -> Result<(), AppError> {
        let db = Database::memory()?;
        let now = chrono::Utc::now().timestamp();
        let old_ts = now - 40 * 86400;

        {
            let conn = crate::database::lock_conn!(db.conn);
            for (request_id, request_model, pricing_model, cost) in [
                ("dim-a", "claude-sonnet-4", "claude-sonnet-4", "0.10"),
                ("dim-b", "claude-haiku-4", "claude-haiku-4", "0.01"),
                ("dim-c", "claude-sonnet-4", "kimi-k2", "0.02"),
            ] {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model, request_model, pricing_model,
                        input_tokens, output_tokens, total_cost_usd,
                        latency_ms, status_code, created_at
                    ) VALUES (?1, 'p1', 'claude', 'kimi-k2', ?2, ?3, 100, 50, ?4, 200, 200, ?5)",
                    rusqlite::params![request_id, request_model, pricing_model, cost, old_ts],
                )?;
            }
        }

        let deleted = db.rollup_and_prune(30)?;
        assert_eq!(deleted, 3);

        let conn = crate::database::lock_conn!(db.conn);
        let mut stmt = conn.prepare(
            "SELECT request_model, pricing_model, request_count, total_cost_usd
             FROM usage_daily_rollups
             WHERE model = 'kimi-k2'
             ORDER BY request_model, pricing_model",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        assert_eq!(
            rows,
            vec![
                (
                    "claude-haiku-4".to_string(),
                    "claude-haiku-4".to_string(),
                    1,
                    "0.01".to_string(),
                ),
                (
                    "claude-sonnet-4".to_string(),
                    "claude-sonnet-4".to_string(),
                    1,
                    "0.1".to_string(),
                ),
                (
                    "claude-sonnet-4".to_string(),
                    "kimi-k2".to_string(),
                    1,
                    "0.02".to_string(),
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn test_rollup_merges_with_existing() -> Result<(), AppError> {
        let db = Database::memory()?;
        let now = chrono::Utc::now().timestamp();
        let old_ts = now - 40 * 86400;

        {
            let conn = crate::database::lock_conn!(db.conn);
            let date_str = Local
                .timestamp_opt(old_ts, 0)
                .single()
                .expect("valid local timestamp")
                .format("%Y-%m-%d")
                .to_string();
            conn.execute(
                "INSERT INTO usage_daily_rollups
                    (date, app_type, provider_id, model, request_count, success_count,
                     input_tokens, output_tokens, total_cost_usd, avg_latency_ms)
                 VALUES (?1, 'claude', 'p1', 'claude-3', 10, 10, 1000, 500, '0.10', 100)",
                [&date_str],
            )?;
            for i in 0..3 {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model,
                        input_tokens, output_tokens, total_cost_usd,
                        latency_ms, status_code, created_at
                    ) VALUES (?1, 'p1', 'claude', 'claude-3', 100, 50, '0.01', 200, 200, ?2)",
                    rusqlite::params![format!("merge-{i}"), old_ts + i as i64],
                )?;
            }
        }

        let deleted = db.rollup_and_prune(30)?;
        assert_eq!(deleted, 3);

        let conn = crate::database::lock_conn!(db.conn);
        let (count, input): (i64, i64) = conn.query_row(
            "SELECT request_count, input_tokens FROM usage_daily_rollups
             WHERE app_type = 'claude' AND provider_id = 'p1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(count, 13, "10 existing + 3 new");
        assert_eq!(input, 1300, "1000 existing + 300 new");
        Ok(())
    }
}
