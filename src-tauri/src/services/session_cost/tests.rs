use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};

use crate::database::Database;
use crate::services::sql_helpers::{INPUT_TOKEN_SEMANTICS_FRESH, INPUT_TOKEN_SEMANTICS_TOTAL};
use crate::session_manager::SessionUsageSummary;

use super::{project_main_connection, project_page, QueryControl, SessionCostIdentity};

fn identity(provider: &str, session_id: &str, source_path: &str) -> SessionCostIdentity {
    SessionCostIdentity {
        provider_id: provider.to_string(),
        session_id: session_id.to_string(),
        source_path: Some(source_path.to_string()),
    }
}

fn target(provider: &str, session_id: &str, source_path: &str) -> SessionCostIdentity {
    identity(provider, session_id, source_path)
}

fn control(seq: u64) -> QueryControl {
    QueryControl {
        active_cost_seq: Arc::new(AtomicU64::new(seq)),
        cost_seq: seq,
        deadline: Instant::now() + Duration::from_secs(2),
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_log_with_nullable_pricing(
    conn: &Connection,
    request_id: &str,
    app_type: &str,
    session_id: &str,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_creation: i64,
    input_semantics: i64,
    cost: f64,
    pricing_model: Option<&str>,
    data_source: &str,
    created_at: i64,
) {
    conn.execute(
        "INSERT INTO proxy_request_logs (
             request_id, provider_id, app_type, model, request_model, pricing_model,
             input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
             input_token_semantics, total_cost_usd, latency_ms, status_code,
             session_id, created_at, data_source
         ) VALUES (
             ?1, '_session_test', ?2, 'test-model', 'test-model', ?3,
             ?4, ?5, ?6, ?7, ?8, ?9, 1, 200, ?10, ?11, ?12
         )",
        params![
            request_id,
            app_type,
            pricing_model,
            input,
            output,
            cache_read,
            cache_creation,
            input_semantics,
            cost.to_string(),
            session_id,
            created_at,
            data_source,
        ],
    )
    .expect("insert request log");
}

#[allow(clippy::too_many_arguments)]
fn insert_log(
    conn: &Connection,
    request_id: &str,
    app_type: &str,
    session_id: &str,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_creation: i64,
    input_semantics: i64,
    cost: f64,
    pricing_model: &str,
    data_source: &str,
    created_at: i64,
) {
    insert_log_with_nullable_pricing(
        conn,
        request_id,
        app_type,
        session_id,
        input,
        output,
        cache_read,
        cache_creation,
        input_semantics,
        cost,
        Some(pricing_model),
        data_source,
        created_at,
    );
}

fn summary<'a>(
    overlays: &'a std::collections::HashMap<SessionCostIdentity, SessionUsageSummary>,
    provider: &str,
    session_id: &str,
    source_path: &str,
) -> &'a SessionUsageSummary {
    overlays
        .get(&identity(provider, session_id, source_path))
        .expect("session overlay")
}

#[test]
fn projection_does_not_depend_on_coverage_settings_table() {
    let db = Database::memory().expect("memory database");
    let conn = db.conn.lock().expect("database lock");
    insert_log(
        &conn,
        "request-structural-error",
        "gemini",
        "session-structural-error",
        12,
        3,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.25,
        "test-model",
        "proxy",
        2_000,
    );
    conn.execute("DROP TABLE settings", [])
        .expect("drop settings table");
    let targets = [target(
        "gemini",
        "session-structural-error",
        "/session-structural-error.json",
    )];

    let overlays = project_main_connection(&conn, &targets, &control(1))
        .expect("cost estimates must not depend on historical coverage evidence");
    assert_eq!(
        summary(
            &overlays,
            "gemini",
            "session-structural-error",
            "/session-structural-error.json"
        )
        .estimated_cost_usd,
        Some(0.25)
    );
}

#[test]
fn unpriced_token_rows_make_cost_unavailable_and_zero_token_errors_do_not_poison() {
    let db = Database::memory().expect("memory database");
    let conn = db.conn.lock().expect("database lock");
    conn.execute(
        "INSERT OR REPLACE INTO model_pricing (
             model_id, display_name, input_cost_per_million,
             output_cost_per_million, cache_read_cost_per_million,
             cache_creation_cost_per_million
         ) VALUES ('free-model', 'Free', '0', '0', '0', '0')",
        [],
    )
    .expect("seed free model");

    insert_log(
        &conn,
        "mixed-priced",
        "gemini",
        "mixed",
        10,
        1,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.5,
        "test-model",
        "proxy",
        2_000,
    );
    insert_log(
        &conn,
        "mixed-unpriced",
        "gemini",
        "mixed",
        20,
        2,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.0,
        "",
        "proxy",
        2_001,
    );
    insert_log(
        &conn,
        "all-unpriced",
        "gemini",
        "all-unpriced",
        20,
        2,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.0,
        "",
        "proxy",
        2_000,
    );
    insert_log(
        &conn,
        "zero-error-priced",
        "gemini",
        "zero-error",
        10,
        1,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.5,
        "test-model",
        "proxy",
        2_000,
    );
    insert_log(
        &conn,
        "zero-error-empty-pricing",
        "gemini",
        "zero-error",
        0,
        0,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.0,
        "",
        "proxy",
        2_001,
    );
    insert_log(
        &conn,
        "true-zero",
        "gemini",
        "true-zero",
        10,
        1,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.0,
        "free-model",
        "proxy",
        2_000,
    );
    insert_log(
        &conn,
        "negative-priced",
        "gemini",
        "negative",
        10,
        1,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.5,
        "test-model",
        "proxy",
        2_000,
    );
    insert_log(
        &conn,
        "negative-invalid",
        "gemini",
        "negative",
        2,
        1,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        -0.1,
        "test-model",
        "proxy",
        2_001,
    );

    let targets = [
        target("gemini", "mixed", "/mixed.json"),
        target("gemini", "all-unpriced", "/all-unpriced.json"),
        target("gemini", "zero-error", "/zero-error.json"),
        target("gemini", "true-zero", "/true-zero.json"),
        target("gemini", "negative", "/negative.json"),
    ];
    let overlays = project_main_connection(&conn, &targets, &control(1)).expect("project database");

    let mixed = summary(&overlays, "gemini", "mixed", "/mixed.json");
    assert_eq!(
        mixed.estimated_cost_usd, None,
        "a partially priced session must not present a misleading estimate"
    );
    assert_eq!(mixed.input_tokens, 30);

    let all_unpriced = summary(&overlays, "gemini", "all-unpriced", "/all-unpriced.json");
    assert_eq!(all_unpriced.estimated_cost_usd, None);
    assert_eq!(all_unpriced.input_tokens, 20);

    let zero_error = summary(&overlays, "gemini", "zero-error", "/zero-error.json");
    assert_eq!(zero_error.estimated_cost_usd, Some(0.5));

    let true_zero = summary(&overlays, "gemini", "true-zero", "/true-zero.json");
    assert_eq!(true_zero.estimated_cost_usd, Some(0.0));

    let negative = summary(&overlays, "gemini", "negative", "/negative.json");
    assert_eq!(
        negative.estimated_cost_usd, None,
        "a negative token-row cost is invalid even when the aggregate remains positive"
    );
}

#[test]
fn write_time_pricing_evidence_proves_zero_cost_without_read_time_model_matching() {
    let db = Database::memory().expect("memory database");
    let conn = db.conn.lock().expect("database lock");
    conn.execute(
        "INSERT OR REPLACE INTO model_pricing (
             model_id, display_name, input_cost_per_million,
             output_cost_per_million, cache_read_cost_per_million,
             cache_creation_cost_per_million
         ) VALUES ('claude-haiku-4.5', 'Claude Haiku', '0', '0', '0', '0')",
        [],
    )
    .expect("seed canonical free-model pricing");
    insert_log(
        &conn,
        "aliased-zero",
        "claude",
        "aliased-zero",
        10,
        1,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.0,
        "anthropic/claude-haiku-4.5",
        "proxy",
        2_000,
    );

    let wanted = target("claude", "aliased-zero", "/aliased-zero.jsonl");
    let overlays = project_main_connection(&conn, std::slice::from_ref(&wanted), &control(1))
        .expect("project explicitly priced zero");

    assert_eq!(
        overlays
            .get(&wanted)
            .expect("zero-cost overlay")
            .estimated_cost_usd,
        Some(0.0),
        "non-empty pricing_model records write-time pricing success; the read path must not \
         re-resolve an alias against today's pricing table"
    );
}

#[test]
fn malformed_stored_cost_fails_closed_even_with_pricing_evidence() {
    let db = Database::memory().expect("memory database");
    let conn = db.conn.lock().expect("database lock");
    conn.execute(
        "INSERT OR REPLACE INTO model_pricing (
             model_id, display_name, input_cost_per_million,
             output_cost_per_million, cache_read_cost_per_million,
             cache_creation_cost_per_million
         ) VALUES ('free-model', 'Free', '0', '0', '0', '0')",
        [],
    )
    .expect("seed free-model pricing");
    conn.execute(
        "INSERT INTO proxy_request_logs (
             request_id, provider_id, app_type, model, request_model, pricing_model,
             input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
             input_token_semantics, total_cost_usd, latency_ms, status_code,
             session_id, created_at, data_source
         ) VALUES (
             'malformed-cost', '_session_test', 'claude', 'free-model', 'free-model',
             'free-model', 10, 1, 0, 0, ?1, 'not-a-decimal', 1, 200,
             'malformed-cost', 2000, 'proxy'
         )",
        [INPUT_TOKEN_SEMANTICS_FRESH],
    )
    .expect("insert malformed stored cost");

    let wanted = target("claude", "malformed-cost", "/malformed-cost.jsonl");
    let overlays = project_main_connection(&conn, std::slice::from_ref(&wanted), &control(1))
        .expect("project malformed cost");

    assert_eq!(
        overlays
            .get(&wanted)
            .expect("token overlay remains available")
            .estimated_cost_usd,
        None,
        "SQLite must not silently CAST malformed cost text into a trusted zero"
    );
}

#[test]
fn zero_token_only_rows_do_not_create_displayable_usage() {
    let db = Database::memory().expect("memory database");
    let conn = db.conn.lock().expect("database lock");
    insert_log(
        &conn,
        "zero-token-only",
        "gemini",
        "zero-token-only",
        0,
        0,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.0,
        "",
        "proxy",
        2_000,
    );
    let wanted = target("gemini", "zero-token-only", "/zero-token-only.json");

    let overlays = project_main_connection(&conn, std::slice::from_ref(&wanted), &control(1))
        .expect("project zero-token-only session");

    assert!(
        !overlays.contains_key(&wanted),
        "without a token-bearing row both Cost and Tokens must remain unavailable"
    );
}

#[test]
fn legacy_null_pricing_keeps_recorded_cost_but_zero_cost_remains_unpriced() {
    let db = Database::memory().expect("memory database");
    let conn = db.conn.lock().expect("database lock");
    for session_id in ["legacy-priced", "legacy-zero"] {
        insert_log_with_nullable_pricing(
            &conn,
            &format!("request-{session_id}"),
            "gemini",
            session_id,
            10,
            2,
            0,
            0,
            INPUT_TOKEN_SEMANTICS_FRESH,
            if session_id == "legacy-priced" {
                0.25
            } else {
                0.0
            },
            None,
            "gemini_session",
            2_000,
        );
    }

    let targets = [
        target("gemini", "legacy-priced", "/legacy-priced.json"),
        target("gemini", "legacy-zero", "/legacy-zero.json"),
    ];
    let overlays = project_main_connection(&conn, &targets, &control(1))
        .expect("project legacy nullable pricing rows");

    let priced = summary(&overlays, "gemini", "legacy-priced", "/legacy-priced.json");
    assert_eq!(priced.estimated_cost_usd, Some(0.25));

    let zero = summary(&overlays, "gemini", "legacy-zero", "/legacy-zero.json");
    assert_eq!(zero.estimated_cost_usd, None);
}

#[test]
fn codex_aliases_merge_without_absorbing_child_threads_and_claude_subagents_do_merge() {
    let db = Database::memory().expect("memory database");
    let conn = db.conn.lock().expect("database lock");
    insert_log(
        &conn,
        "codex-root-direct",
        "codex",
        "root",
        10,
        1,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.1,
        "test-model",
        "proxy",
        2_000,
    );
    insert_log(
        &conn,
        "codex-root-prefixed",
        "codex",
        "codex_root",
        20,
        2,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.2,
        "test-model",
        "proxy",
        2_001,
    );
    insert_log(
        &conn,
        "codex-child",
        "codex",
        "child-thread",
        100,
        10,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.9,
        "test-model",
        "codex_session",
        2_002,
    );

    insert_log(
        &conn,
        "claude-root",
        "claude",
        "claude-root",
        10,
        1,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.1,
        "test-model",
        "session_log",
        2_000,
    );
    insert_log(
        &conn,
        "claude-subagent",
        "claude",
        "claude-root",
        20,
        2,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.2,
        "test-model",
        "session_log",
        2_001,
    );

    let targets = [
        target("codex", "root", "/codex-root.jsonl"),
        target("claude", "claude-root", "/claude-root.jsonl"),
    ];
    let overlays = project_main_connection(&conn, &targets, &control(1)).expect("project database");

    let codex = summary(&overlays, "codex", "root", "/codex-root.jsonl");
    assert!((codex.estimated_cost_usd.expect("codex cost") - 0.3).abs() < 1e-9);
    assert_eq!(codex.input_tokens, 30);

    let claude = summary(&overlays, "claude", "claude-root", "/claude-root.jsonl");
    assert!((claude.estimated_cost_usd.expect("claude cost") - 0.3).abs() < 1e-9);
    assert_eq!(claude.input_tokens, 30);
}

#[test]
fn duplicate_visible_source_paths_are_ambiguous_and_receive_no_overlay() {
    let db = Database::memory().expect("memory database");
    let conn = db.conn.lock().expect("database lock");
    insert_log(
        &conn,
        "ambiguous",
        "codex",
        "same-id",
        10,
        1,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.1,
        "test-model",
        "proxy",
        2_000,
    );
    let targets = [
        target("codex", "same-id", "/one.jsonl"),
        target("codex", "same-id", "/two.jsonl"),
    ];

    let overlays = project_main_connection(&conn, &targets, &control(1)).expect("project database");
    assert!(overlays.is_empty());
}

#[test]
fn session_projection_matches_usage_page_bucket_and_cost_semantics() {
    let db = Database::memory().expect("memory database");
    let conn = db.conn.lock().expect("database lock");
    insert_log(
        &conn,
        "fresh",
        "codex",
        "parity",
        100,
        20,
        30,
        10,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.4,
        "test-model",
        "proxy",
        2_000,
    );
    insert_log(
        &conn,
        "total-inclusive",
        "codex",
        "parity",
        200,
        40,
        50,
        20,
        INPUT_TOKEN_SEMANTICS_TOTAL,
        0.6,
        "test-model",
        "proxy",
        2_001,
    );
    let target = target("codex", "parity", "/parity.jsonl");
    let overlays =
        project_main_connection(&conn, &[target], &control(1)).expect("project main database");
    let projected = summary(&overlays, "codex", "parity", "/parity.jsonl");
    let usage = crate::cli::tui::data::load_usage_summary(&conn, "codex", 0, 3_000)
        .expect("load Usage page summary");

    assert_eq!(projected.input_tokens, usage.input_tokens);
    assert_eq!(projected.output_tokens, usage.output_tokens);
    assert_eq!(projected.cache_read_tokens, usage.cache_read_tokens);
    assert_eq!(projected.cache_creation_tokens, usage.cache_creation_tokens);
    assert_eq!(projected.estimated_cost_usd, Some(usage.total_cost_usd));
}

#[test]
fn projection_connection_performs_no_writes() {
    let db = Database::memory().expect("memory database");
    let conn = db.conn.lock().expect("database lock");
    insert_log(
        &conn,
        "readonly",
        "gemini",
        "readonly",
        1,
        1,
        0,
        0,
        INPUT_TOKEN_SEMANTICS_FRESH,
        0.1,
        "test-model",
        "proxy",
        2_000,
    );
    let counts_before = conn
        .query_row(
            "SELECT
                 (SELECT COUNT(*) FROM proxy_request_logs),
                 (SELECT COUNT(*) FROM settings),
                 (SELECT COUNT(*) FROM session_log_sync)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .expect("counts before projection");
    conn.pragma_update(None, "query_only", true)
        .expect("enable query-only guard");
    let overlays = project_main_connection(
        &conn,
        &[target("gemini", "readonly", "/readonly.json")],
        &control(1),
    )
    .expect("read-only projection");
    assert_eq!(overlays.len(), 1);
    let counts_after = conn
        .query_row(
            "SELECT
                 (SELECT COUNT(*) FROM proxy_request_logs),
                 (SELECT COUNT(*) FROM settings),
                 (SELECT COUNT(*) FROM session_log_sync)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .expect("counts after projection");
    assert_eq!(counts_after, counts_before);
}

#[test]
fn main_projection_query_plan_is_driven_by_the_session_index() {
    let db = Database::memory().expect("memory database");
    let conn = db.conn.lock().expect("database lock");
    let targets = [target("codex", "indexed", "/indexed.jsonl")];

    let plan = super::projection::explain_main_query_plan(&conn, &targets)
        .expect("explain main projection");
    let joined = plan.join("\n");
    assert!(
        joined.contains("idx_request_logs_session"),
        "wanted IDs must probe the session index instead of scanning logs:\n{joined}"
    );
}

#[test]
#[serial_test::serial(home_settings)]
fn missing_and_future_schema_databases_degrade_to_empty_overlays_without_initializing() {
    for future_schema in [false, true] {
        let temp = tempfile::tempdir().expect("isolated config");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let db_path = crate::database::database_path().expect("database path");
        if future_schema {
            std::fs::create_dir_all(db_path.parent().expect("database parent"))
                .expect("create database parent");
            let conn = Connection::open(&db_path).expect("create future database");
            Database::create_tables_on_conn(&conn).expect("create current tables");
            Database::set_user_version(&conn, crate::database::SCHEMA_VERSION + 1)
                .expect("set future schema");
        }

        let targets = [target("gemini", "missing", "/missing.json")];
        assert!(project_page(&targets, &control(1)).is_empty());
        if !future_schema {
            assert!(
                !db_path.exists(),
                "a missing database must not be created by projection"
            );
        } else {
            let conn = Connection::open(&db_path).expect("reopen future database");
            assert_eq!(
                Database::get_user_version(&conn).expect("future version"),
                crate::database::SCHEMA_VERSION + 1
            );
        }
    }
}

#[test]
#[serial_test::serial(home_settings)]
fn locked_main_database_degrades_within_the_overlay_busy_budget() {
    let temp = tempfile::tempdir().expect("isolated config");
    let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
    let db = Database::init().expect("initialize isolated database");
    let db_path = crate::database::database_path().expect("database path");
    drop(db);

    let locker = Connection::open(&db_path).expect("open lock connection");
    locker
        .pragma_update(None, "journal_mode", "DELETE")
        .expect("use reader-blocking journal mode");
    locker
        .execute_batch("BEGIN EXCLUSIVE;")
        .expect("hold exclusive database lock");

    let probe = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("open read-only probe");
    probe
        .busy_timeout(Duration::from_millis(1))
        .expect("set probe timeout");
    assert!(
        Database::get_user_version(&probe).is_err(),
        "fixture must actually block snapshot reads"
    );

    let started = Instant::now();
    let targets = [target("gemini", "locked", "/locked.json")];
    assert!(project_page(&targets, &control(1)).is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "busy projection must respect the 250ms budget, elapsed={:?}",
        started.elapsed()
    );
    locker
        .execute_batch("ROLLBACK;")
        .expect("release exclusive database lock");
}

#[test]
fn replacement_sequence_interrupts_an_inflight_sqlite_statement() {
    let active_cost_seq = Arc::new(AtomicU64::new(1));
    let worker_seq = Arc::clone(&active_cost_seq);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();

    let handle = std::thread::spawn(move || {
        let conn = Connection::open_in_memory().expect("long-query connection");
        let control = QueryControl {
            active_cost_seq: worker_seq,
            cost_seq: 1,
            deadline: Instant::now() + Duration::from_secs(5),
        };
        control.install_progress_handler(&conn);
        started_tx.send(()).expect("announce long query");
        let result = conn.query_row(
            "WITH RECURSIVE counter(value) AS (
                 VALUES(0)
                 UNION ALL
                 SELECT value + 1 FROM counter WHERE value < 1000000000
             )
             SELECT SUM(value) FROM counter",
            [],
            |row| row.get::<_, i64>(0),
        );
        QueryControl::clear_progress_handler(&conn);
        finished_tx.send(result).expect("report long query");
    });

    started_rx.recv().expect("long query started");
    std::thread::sleep(Duration::from_millis(25));
    assert!(
        finished_rx.try_recv().is_err(),
        "fixture query must still be executing before its replacement arrives"
    );
    let cancelled_at = Instant::now();
    active_cost_seq.store(2, Ordering::Release);
    let result = finished_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("SQLite progress handler should interrupt promptly");
    assert!(result.is_err(), "replacement must interrupt the old SQL");
    assert!(
        cancelled_at.elapsed() < Duration::from_millis(500),
        "replacement cancellation should not wait for the two-second deadline"
    );
    handle.join().expect("long-query worker");
}

#[test]
fn openclaw_has_no_cost_projection_and_summary_cost_is_explicitly_estimated() {
    let openclaw = target("openclaw", "openclaw-session", "/openclaw.jsonl");
    let overlays = project_page(&[openclaw], &control(1));
    assert!(overlays.is_empty());

    let estimated = SessionUsageSummary {
        input_tokens: 1,
        output_tokens: 2,
        cache_read_tokens: 3,
        cache_creation_tokens: 4,
        estimated_cost_usd: Some(0.5),
    };
    assert_eq!(estimated.estimated_cost_usd, Some(0.5));
}
