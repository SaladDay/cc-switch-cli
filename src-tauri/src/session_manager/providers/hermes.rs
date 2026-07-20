use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

#[cfg(test)]
use std::fs::File;

use rusqlite::{types::ValueRef, Connection, OptionalExtension};
use serde_json::Value;

use crate::hermes_config::get_hermes_dir;
use crate::session_manager::cache::{self, FileScanTarget};
use crate::session_manager::scan_cache_store::ScanCacheStore;
use crate::session_manager::{
    SearchSnippet, SessionMessage, SessionMessageBatch, SessionMessageBatchBuilder, SessionMeta,
    SessionSearchHit, SESSION_MESSAGE_PREVIEW_MAX_MESSAGES,
    SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES,
};

use super::utils::{
    build_snippet_cancellable, extract_text, file_modified_ms, parse_timestamp_to_ms,
    read_head_tail_lines_bounded, truncate_summary, visit_bounded_lines_cancellable,
    visit_bounded_lines_cancellable_with_status, with_sqlite_cancellation, MAX_METADATA_FILE_BYTES,
    MAX_METADATA_LINE_BYTES, TITLE_MAX_CHARS,
};

const PROVIDER_ID: &str = "hermes";

fn get_hermes_db_path() -> PathBuf {
    get_hermes_dir().join("state.db")
}

fn get_hermes_sessions_dir() -> PathBuf {
    get_hermes_dir().join("sessions")
}

/// Scan sessions from both SQLite database and JSONL transcript files,
/// with SQLite taking precedence on ID conflicts.
pub fn scan_sessions() -> Vec<SessionMeta> {
    merge_sqlite_jsonl(scan_sessions_sqlite(), scan_sessions_jsonl())
}

/// Cache-aware scan: the JSONL transcripts go through the persistent file cache,
/// while the SQLite database is always queried fresh. Merged with SQLite taking
/// precedence — identical semantics to `scan_sessions`.
pub(crate) fn scan_sessions_cached(store: &ScanCacheStore, force: bool) -> Vec<SessionMeta> {
    let jsonl_sessions = cache::scan_provider_cached(
        store,
        PROVIDER_ID,
        scan_targets(),
        force,
        parse_jsonl_session,
        |_| true,
    );
    merge_sqlite_jsonl(scan_sessions_sqlite(), jsonl_sessions)
}

pub(crate) fn scan_sessions_progressive(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(&SessionMeta),
) -> Vec<SessionMeta> {
    let targets = scan_targets();
    let jsonl_sessions = match store {
        Some(store) => cache::scan_provider_cached_progressive(
            store,
            PROVIDER_ID,
            targets,
            force,
            parse_jsonl_session,
            |_| true,
            &mut *on_session,
        ),
        None => cache::scan_provider_uncached_progressive(
            targets,
            parse_jsonl_session,
            &mut *on_session,
        ),
    };

    let sqlite_sessions = scan_sessions_sqlite();
    for session in &sqlite_sessions {
        on_session(session);
    }
    merge_sqlite_jsonl(sqlite_sessions, jsonl_sessions)
}

pub(crate) fn scan_sessions_progressive_cancellable(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(&SessionMeta),
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionMeta>> {
    let targets = scan_targets_cancellable(is_cancelled)?;
    let jsonl_sessions = match store {
        Some(store) => cache::scan_provider_cached_progressive_cancellable(
            store,
            PROVIDER_ID,
            targets,
            force,
            parse_jsonl_session,
            |_| true,
            &mut *on_session,
            is_cancelled,
        )?,
        None => cache::scan_provider_uncached_progressive_cancellable(
            targets,
            parse_jsonl_session,
            &mut *on_session,
            is_cancelled,
        )?,
    };
    let sqlite_sessions = scan_sessions_sqlite_cancellable(is_cancelled)?;
    for session in &sqlite_sessions {
        if is_cancelled() {
            return None;
        }
        on_session(session);
    }
    if is_cancelled() {
        None
    } else {
        merge_sqlite_jsonl_cancellable(sqlite_sessions, jsonl_sessions, is_cancelled)
    }
}

pub(crate) fn stream_sessions_cancellable(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(SessionMeta) -> ControlFlow<()>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<cache::StreamScanStats, cache::StreamScanStop> {
    let db = open_stream_database()?;
    let plan = db.as_ref().map(HermesStreamPlan::discover).transpose()?;
    let mut stats = cache::StreamScanStats::default();
    if let (Some(conn), Some(plan)) = (db.as_ref(), plan.as_ref()) {
        stream_sqlite_sessions(conn, plan, on_session, is_cancelled, &mut stats)?;
    }

    let sessions_dir = get_hermes_sessions_dir();
    let mut duplicates = 0usize;
    let duplicate_lookup_failed = std::cell::Cell::new(false);
    let db_source = format!("sqlite:{}", get_hermes_db_path().display());
    let mut json_sink = |meta: SessionMeta| {
        let duplicate = match (db.as_ref(), plan.as_ref()) {
            (Some(conn), Some(plan)) => {
                match plan.decodable_recent_contains(conn, &meta.session_id, &db_source) {
                    Ok(duplicate) => duplicate,
                    Err(()) => {
                        duplicate_lookup_failed.set(true);
                        return ControlFlow::Break(());
                    }
                }
            }
            _ => false,
        };
        if duplicate {
            duplicates = duplicates.saturating_add(1);
            return ControlFlow::Continue(());
        }
        on_session(meta)
    };
    let file_result = cache::stream_file_provider_cancellable(
        store,
        PROVIDER_ID,
        force,
        |path| {
            if is_cancelled() {
                return Err(cache::StreamScanStop::Cancelled);
            }
            let meta = parse_jsonl_session_authoritative(path)?;
            if is_cancelled() {
                Err(cache::StreamScanStop::Cancelled)
            } else {
                Ok(meta)
            }
        },
        |_| true,
        cache::stat_target,
        move |on_target, cancel| {
            if cancel() {
                return Err(cache::StreamScanStop::Cancelled);
            }
            let entries = match std::fs::read_dir(&sessions_dir) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => {
                    log::warn!(
                        "authoritative Hermes session walk failed at {}: {error}",
                        sessions_dir.display()
                    );
                    return Err(cache::StreamScanStop::Incomplete);
                }
            };
            for entry in entries {
                if cancel() {
                    return Err(cache::StreamScanStop::Cancelled);
                }
                let entry = entry.map_err(|error| {
                    log::warn!(
                        "authoritative Hermes directory entry failed at {}: {error}",
                        sessions_dir.display()
                    );
                    cache::StreamScanStop::Incomplete
                })?;
                let path = entry.path();
                if !matches!(
                    path.extension().and_then(|value| value.to_str()),
                    Some("jsonl" | "json")
                ) {
                    continue;
                }
                let kind = entry.file_type().map_err(|error| {
                    log::warn!(
                        "authoritative Hermes session type failed at {}: {error}",
                        path.display()
                    );
                    cache::StreamScanStop::Incomplete
                })?;
                let regular = kind.is_file()
                    || (kind.is_symlink()
                        && std::fs::metadata(&path)
                            .map(|metadata| metadata.is_file())
                            .map_err(|error| {
                                log::warn!(
                                    "authoritative Hermes symlink stat failed at {}: {error}",
                                    path.display()
                                );
                                cache::StreamScanStop::Incomplete
                            })?);
                if !regular {
                    continue;
                }
                let target = cache::stat_target_strict(&path)
                    .map_err(|error| {
                        log::warn!(
                            "authoritative Hermes session stat failed at {}: {error}",
                            path.display()
                        );
                        cache::StreamScanStop::Incomplete
                    })?
                    .ok_or(cache::StreamScanStop::Incomplete)?;
                on_target(target)?;
            }
            Ok(())
        },
        &mut json_sink,
        is_cancelled,
    );
    drop(json_sink);
    if duplicate_lookup_failed.get() {
        return Err(cache::StreamScanStop::Incomplete);
    }
    let mut file_stats = file_result?;
    file_stats.emitted = file_stats.emitted.saturating_sub(duplicates);
    stats.merge(file_stats);
    Ok(stats)
}

fn open_stream_database() -> Result<Option<Connection>, cache::StreamScanStop> {
    let path = get_hermes_db_path();
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(cache::StreamScanStop::Incomplete),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            log::warn!(
                "authoritative Hermes database stat failed at {}: {error}",
                path.display()
            );
            return Err(cache::StreamScanStop::Incomplete);
        }
    }
    let conn = Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        log::warn!(
            "authoritative Hermes database open failed at {}: {error}",
            path.display()
        );
        cache::StreamScanStop::Incomplete
    })?;
    let exists = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='sessions')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| {
            log::warn!("authoritative Hermes schema inspection failed: {error}");
            cache::StreamScanStop::Incomplete
        })?;
    if exists {
        Ok(Some(conn))
    } else {
        Ok(None)
    }
}

#[derive(Debug)]
struct HermesStreamPlan {
    select_list: String,
}

impl HermesStreamPlan {
    fn discover(conn: &Connection) -> Result<Self, cache::StreamScanStop> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(sessions)")
            .map_err(|error| {
                log::warn!("authoritative Hermes column inspection prepare failed: {error}");
                cache::StreamScanStop::Incomplete
            })?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| {
                log::warn!("authoritative Hermes column inspection failed: {error}");
                cache::StreamScanStop::Incomplete
            })?;
        let mut columns = std::collections::HashSet::new();
        for row in rows {
            columns.insert(row.map_err(|error| {
                log::warn!("authoritative Hermes column decode failed: {error}");
                cache::StreamScanStop::Incomplete
            })?);
        }
        if !columns.contains("id") {
            return Err(cache::StreamScanStop::Incomplete);
        }
        let bounded_optional = |names: &[&'static str]| {
            names
                .iter()
                .copied()
                .find(|name| columns.contains(*name))
                .map_or_else(
                    || "NULL".to_string(),
                    |name| {
                        format!(
                        "CASE WHEN length(CAST(\"{name}\" AS BLOB)) <= {MAX_METADATA_LINE_BYTES} \
                         THEN \"{name}\" ELSE NULL END"
                    )
                    },
                )
        };
        Ok(Self {
            // Only metadata fields are selected. In particular, a flexible
            // upstream sessions table may contain transcript/blob columns that
            // must never enter the list scanner's memory. Text is bounded in
            // SQL before rusqlite materializes it in a Rust String.
            select_list: format!(
                "CASE WHEN length(CAST(\"id\" AS BLOB)) <= {MAX_METADATA_LINE_BYTES} \
                     THEN CAST(\"id\" AS TEXT) ELSE printf('oversized-rowid-%lld', rowid) END, \
                 {}, {}, {}, {}",
                bounded_optional(&["title"]),
                bounded_optional(&["cwd", "directory"]),
                bounded_optional(&["started_at", "created_at"]),
                bounded_optional(&["ended_at", "updated_at"]),
            ),
        })
    }

    fn decode_row(
        &self,
        row: &rusqlite::Row<'_>,
        db_source: &str,
    ) -> rusqlite::Result<SessionMeta> {
        let session_id: String = row.get(0)?;
        let title = row.get::<_, Option<String>>(1)?.and_then(|value| {
            (!value.is_empty()).then(|| truncate_summary(&value, TITLE_MAX_CHARS))
        });
        let cwd = row
            .get::<_, Option<String>>(2)?
            .filter(|value| !value.is_empty());
        let started_at = sqlite_value_to_json(row.get_ref(3)?).and_then(|value| {
            (!value.is_null())
                .then(|| parse_timestamp_to_ms(&value))
                .flatten()
        });
        let ended_at = sqlite_value_to_json(row.get_ref(4)?).and_then(|value| {
            (!value.is_null())
                .then(|| parse_timestamp_to_ms(&value))
                .flatten()
        });
        Ok(SessionMeta {
            provider_id: PROVIDER_ID.to_string(),
            session_id: session_id.clone(),
            title,
            summary: None,
            project_dir: cwd,
            created_at: started_at,
            last_active_at: ended_at.or(started_at),
            source_path: Some(format!("{db_source}#{session_id}")),
            resume_command: None,
        })
    }

    fn decodable_recent_contains(
        &self,
        conn: &Connection,
        session_id: &str,
        db_source: &str,
    ) -> Result<bool, ()> {
        let sql = format!(
            "SELECT {} FROM sessions
             WHERE id = ?1
               AND rowid IN (SELECT rowid FROM sessions ORDER BY rowid DESC LIMIT 500)
             LIMIT 1",
            self.select_list
        );
        conn.query_row(&sql, [session_id], |row| self.decode_row(row, db_source))
            .optional()
            .map(|meta| meta.is_some())
            .map_err(|error| {
                log::warn!(
                    "authoritative Hermes duplicate decode failed for {session_id}: {error}"
                );
            })
    }
}

fn sqlite_value_to_json(value: ValueRef<'_>) -> Option<Value> {
    match value {
        ValueRef::Null => Some(Value::Null),
        ValueRef::Integer(value) => Some(Value::Number(value.into())),
        ValueRef::Real(value) => serde_json::Number::from_f64(value).map(Value::Number),
        ValueRef::Text(value) => Some(Value::String(String::from_utf8_lossy(value).into_owned())),
        ValueRef::Blob(_) => None,
    }
}

fn stream_sqlite_sessions(
    conn: &Connection,
    plan: &HermesStreamPlan,
    on_session: &mut dyn FnMut(SessionMeta) -> ControlFlow<()>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    stats: &mut cache::StreamScanStats,
) -> Result<(), cache::StreamScanStop> {
    if is_cancelled() {
        return Err(cache::StreamScanStop::Cancelled);
    }
    let sql = format!(
        "SELECT {} FROM sessions ORDER BY rowid DESC LIMIT 500",
        plan.select_list
    );
    let mut stmt = conn.prepare(&sql).map_err(|error| {
        log::warn!("authoritative Hermes session query prepare failed: {error}");
        cache::StreamScanStop::Incomplete
    })?;
    let source = format!("sqlite:{}", get_hermes_db_path().display());
    let rows = stmt
        .query_map([], |row| plan.decode_row(row, &source))
        .map_err(|error| {
            log::warn!("authoritative Hermes session query failed: {error}");
            cache::StreamScanStop::Incomplete
        })?;
    for row in rows {
        if is_cancelled() {
            return Err(cache::StreamScanStop::Cancelled);
        }
        let meta = row.map_err(|error| {
            log::warn!("authoritative Hermes session row decode failed: {error}");
            cache::StreamScanStop::Incomplete
        })?;
        stats.discovered = stats.discovered.saturating_add(1);
        if on_session(meta).is_break() {
            return Err(cache::StreamScanStop::SinkStopped);
        }
        stats.emitted = stats.emitted.saturating_add(1);
    }
    Ok(())
}

/// Merge SQLite and JSONL sessions, keeping the SQLite row when the same
/// `session_id` exists in both.
fn merge_sqlite_jsonl(
    sqlite_sessions: Vec<SessionMeta>,
    jsonl_sessions: Vec<SessionMeta>,
) -> Vec<SessionMeta> {
    if sqlite_sessions.is_empty() {
        return jsonl_sessions;
    }
    if jsonl_sessions.is_empty() {
        return sqlite_sessions;
    }

    let sqlite_ids: std::collections::HashSet<String> = sqlite_sessions
        .iter()
        .map(|s| s.session_id.clone())
        .collect();

    let mut merged = sqlite_sessions;
    for s in jsonl_sessions {
        if !sqlite_ids.contains(&s.session_id) {
            merged.push(s);
        }
    }
    merged
}

fn merge_sqlite_jsonl_cancellable(
    sqlite_sessions: Vec<SessionMeta>,
    jsonl_sessions: Vec<SessionMeta>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionMeta>> {
    if is_cancelled() {
        return None;
    }
    if sqlite_sessions.is_empty() {
        return Some(jsonl_sessions);
    }
    if jsonl_sessions.is_empty() {
        return Some(sqlite_sessions);
    }
    let mut sqlite_ids = std::collections::HashSet::with_capacity(sqlite_sessions.len());
    for session in &sqlite_sessions {
        if is_cancelled() {
            return None;
        }
        sqlite_ids.insert(session.session_id.clone());
    }
    let mut merged = sqlite_sessions;
    for session in jsonl_sessions {
        if is_cancelled() {
            return None;
        }
        if !sqlite_ids.contains(&session.session_id) {
            merged.push(session);
        }
    }
    Some(merged)
}

/// Collect the flat `sessions/` directory (both `.jsonl` and `.json`), statting
/// each file once. Matches `scan_sessions_jsonl`'s non-recursive traversal.
fn scan_targets() -> Vec<FileScanTarget> {
    scan_targets_cancellable(&|| false).expect("non-cancellable target scan cannot stop")
}

fn scan_targets_cancellable(
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<FileScanTarget>> {
    let sessions_dir = get_hermes_sessions_dir();
    let mut targets = Vec::new();
    if is_cancelled() {
        return None;
    }
    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(entries) => entries,
        Err(_) => return Some(targets),
    };
    for entry in entries.flatten() {
        if is_cancelled() {
            return None;
        }
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if ext != Some("jsonl") && ext != Some("json") {
            continue;
        }
        if let Some(target) = cache::stat_target(&path) {
            targets.push(target);
        }
    }
    Some(targets)
}

// ── SQLite scanning ─────────────────────────────────────────────────

fn scan_sessions_sqlite() -> Vec<SessionMeta> {
    scan_sessions_sqlite_cancellable(&|| false)
        .expect("non-cancellable SQLite session scan cannot stop")
}

fn scan_sessions_sqlite_cancellable(
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionMeta>> {
    if is_cancelled() {
        return None;
    }
    let conn = match open_stream_database() {
        Ok(Some(conn)) => conn,
        Ok(None) => return Some(Vec::new()),
        Err(cache::StreamScanStop::Cancelled) => return None,
        Err(_) => return Some(Vec::new()),
    };
    let plan = match HermesStreamPlan::discover(&conn) {
        Ok(plan) => plan,
        Err(_) => return Some(Vec::new()),
    };
    let mut sessions = Vec::new();
    let mut stats = cache::StreamScanStats::default();
    let result = stream_sqlite_sessions(
        &conn,
        &plan,
        &mut |meta| {
            sessions.push(meta);
            ControlFlow::Continue(())
        },
        is_cancelled,
        &mut stats,
    );
    match result {
        Ok(()) => Some(sessions),
        Err(cache::StreamScanStop::Cancelled) => None,
        Err(_) => Some(Vec::new()),
    }
}

/// Load messages from the Hermes SQLite database.
pub fn load_messages_sqlite(source: &str) -> Result<SessionMessageBatch, String> {
    load_messages_sqlite_cancellable(source, &|| false)
}

pub(crate) fn load_messages_sqlite_cancellable(
    source: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<SessionMessageBatch, String> {
    let (db_path, session_id) = parse_sqlite_source(source)
        .ok_or_else(|| format!("Invalid SQLite source reference: {source}"))?;

    if is_cancelled() {
        return Err("Session message preview was cancelled".to_string());
    }

    let conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("Failed to open Hermes database: {e}"))?;

    with_sqlite_cancellation(&conn, is_cancelled, || {
        load_messages_sqlite_from_connection(&conn, &session_id, is_cancelled)
    })
}

fn load_messages_sqlite_from_connection(
    conn: &Connection,
    session_id: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<SessionMessageBatch, String> {
    // Read one lookahead row from the newest end. The fixed SQL substrings
    // bound rusqlite allocations; Rust restores chronological display order.
    let query = format!(
        "SELECT
            substr(CAST(role AS TEXT), 1, 256),
            length(CAST(role AS BLOB)),
            substr(CAST(content AS TEXT), 1, {SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES}),
            length(CAST(content AS BLOB)),
            created_at
         FROM messages
         WHERE session_id = ?1
         ORDER BY created_at DESC
         LIMIT {}",
        SESSION_MESSAGE_PREVIEW_MAX_MESSAGES + 1
    );

    let mut stmt = conn
        .prepare(&query)
        .map_err(|e| format!("Failed to prepare messages query: {e}"))?;

    let rows = stmt
        .query_map([session_id], |row| {
            let role: Option<String> = row.get(0)?;
            let role_bytes: Option<i64> = row.get(1)?;
            let content: Option<String> = row.get(2)?;
            let content_bytes: Option<i64> = row.get(3)?;
            let ts: Option<i64> = row.get(4).ok();
            Ok((role, role_bytes, content, content_bytes, ts))
        })
        .map_err(|e| format!("Failed to query messages: {e}"))?;

    let mut newest = Vec::with_capacity(SESSION_MESSAGE_PREVIEW_MAX_MESSAGES + 1);
    let mut truncated = false;
    for row in rows {
        if is_cancelled() {
            return Err("Session message preview was cancelled".to_string());
        }
        let (role, role_bytes, content, content_bytes, ts) =
            row.map_err(|error| format!("Failed to decode message row: {error}"))?;
        let (Some(role), Some(content)) = (role, content) else {
            truncated = true;
            continue;
        };
        if content.trim().is_empty() {
            continue;
        }
        truncated |= role_bytes.is_some_and(|bytes| bytes > 256)
            || content_bytes
                .is_some_and(|bytes| bytes > SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES as i64);
        let ts_ms = ts.and_then(|v| parse_timestamp_to_ms(&Value::Number(v.into())));
        newest.push(SessionMessage {
            role,
            content,
            ts: ts_ms,
        });
    }

    if newest.len() > SESSION_MESSAGE_PREVIEW_MAX_MESSAGES {
        newest.truncate(SESSION_MESSAGE_PREVIEW_MAX_MESSAGES);
        truncated = true;
    }
    let mut batch = SessionMessageBatchBuilder::new();
    if truncated {
        batch.mark_truncated();
    }
    for message in newest {
        if is_cancelled() {
            return Err("Session message preview was cancelled".to_string());
        }
        if batch.push(message).is_break() {
            break;
        }
    }
    let mut batch = batch.finish();
    batch.messages.reverse();
    Ok(batch)
}

/// Delete a session from the Hermes SQLite database.
pub fn delete_session_sqlite(session_id: &str, source: &str) -> Result<bool, String> {
    let (db_path, ref_session_id) = parse_sqlite_source(source)
        .ok_or_else(|| format!("Invalid SQLite source reference: {source}"))?;
    let db_path = db_path
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize Hermes database path: {e}"))?;
    let expected_db_path = get_hermes_db_path()
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize expected Hermes database path: {e}"))?;

    if ref_session_id != session_id {
        return Err(format!(
            "Hermes SQLite session ID mismatch: expected {session_id}, found {ref_session_id}"
        ));
    }
    if db_path != expected_db_path {
        return Err("SQLite path does not match expected Hermes database".to_string());
    }

    let conn =
        Connection::open(&db_path).map_err(|e| format!("Failed to open Hermes database: {e}"))?;

    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("Failed to begin transaction: {e}"))?;

    // Delete messages first (child records)
    let _ = tx.execute("DELETE FROM messages WHERE session_id = ?1", [session_id]);

    let deleted = tx
        .execute("DELETE FROM sessions WHERE id = ?1", [session_id])
        .map_err(|e| format!("Failed to delete Hermes session: {e}"))?;

    tx.commit()
        .map_err(|e| format!("Failed to commit session deletion: {e}"))?;

    Ok(deleted > 0)
}

fn parse_sqlite_source(source: &str) -> Option<(PathBuf, String)> {
    let rest = source.strip_prefix("sqlite:")?;
    let hash_pos = rest.rfind('#')?;
    let db_path = PathBuf::from(&rest[..hash_pos]);
    let session_id = rest[hash_pos + 1..].to_string();
    if session_id.is_empty() {
        return None;
    }
    Some((db_path, session_id))
}

// ── JSONL scanning ──────────────────────────────────────────────────

fn scan_sessions_jsonl() -> Vec<SessionMeta> {
    let sessions_dir = get_hermes_sessions_dir();
    if !sessions_dir.exists() {
        return Vec::new();
    }

    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if ext != Some("jsonl") && ext != Some("json") {
            continue;
        }
        if let Some(meta) = parse_jsonl_session(&path) {
            sessions.push(meta);
        }
    }
    sessions
}

fn parse_jsonl_session(path: &Path) -> Option<SessionMeta> {
    parse_jsonl_session_authoritative(path).ok().flatten()
}

fn parse_jsonl_session_authoritative(
    path: &Path,
) -> Result<Option<SessionMeta>, cache::StreamScanStop> {
    // Read head (metadata + first user message) and tail (last timestamp)
    let (head, tail) = read_head_tail_lines_bounded(path, 30, 10).map_err(|error| {
        log::warn!(
            "authoritative Hermes metadata read failed at {}: {error}",
            path.display()
        );
        cache::StreamScanStop::Incomplete
    })?;
    Ok(parse_jsonl_session_lines(path, &head, &tail))
}

fn parse_jsonl_session_lines(path: &Path, head: &[String], tail: &[String]) -> Option<SessionMeta> {
    let mut first_user_msg: Option<String> = None;
    let mut first_ts: Option<i64> = None;
    let mut last_ts: Option<i64> = None;
    let mut session_id: Option<String> = None;
    let mut title: Option<String> = None;
    let mut cwd: Option<String> = None;

    // Process head lines for metadata and first user message
    for line in head {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let ts = value
            .get("timestamp")
            .or_else(|| value.get("ts"))
            .and_then(parse_timestamp_to_ms);

        if first_ts.is_none() {
            first_ts = ts;
        }
        last_ts = ts.or(last_ts);

        let line_type = value.get("type").and_then(Value::as_str).unwrap_or("");

        // Extract session metadata from session-type lines
        if line_type == "session" || line_type == "init" {
            if session_id.is_none() {
                session_id = value
                    .get("id")
                    .or_else(|| value.get("sessionId"))
                    .and_then(Value::as_str)
                    .map(|s| s.to_string());
            }
            if title.is_none() {
                title = value
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
            }
            if cwd.is_none() {
                cwd = value
                    .get("cwd")
                    .or_else(|| value.get("directory"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
            }
        }

        if first_user_msg.is_none() {
            let role = value
                .get("role")
                .or_else(|| value.get("message").and_then(|m| m.get("role")))
                .and_then(Value::as_str);

            if role == Some("user") {
                let content = value
                    .get("content")
                    .or_else(|| value.get("message").and_then(|m| m.get("content")));
                if let Some(c) = content {
                    let text = extract_text(c);
                    if !text.trim().is_empty() {
                        first_user_msg = Some(truncate_summary(&text, TITLE_MAX_CHARS).to_string());
                    }
                }
            }
        }
    }

    // Process tail lines for the most recent timestamp
    for line in tail.iter().rev() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ts = value
            .get("timestamp")
            .or_else(|| value.get("ts"))
            .and_then(parse_timestamp_to_ms);
        if let Some(t) = ts {
            last_ts = Some(t);
            break;
        }
    }

    // Fall back to filename as session ID
    let session_id = session_id.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    });

    let source_path = path.to_string_lossy().to_string();
    let fallback_time = file_modified_ms(path);
    let fallback_title = path
        .file_stem()
        .and_then(|value| value.to_str())
        .map(|value| truncate_summary(value, TITLE_MAX_CHARS));

    Some(SessionMeta {
        provider_id: PROVIDER_ID.to_string(),
        session_id,
        title: title.or_else(|| first_user_msg.clone()).or(fallback_title),
        summary: first_user_msg,
        project_dir: cwd,
        created_at: first_ts.or(fallback_time),
        last_active_at: last_ts.or(first_ts).or(fallback_time),
        source_path: Some(source_path),
        resume_command: None,
    })
}

/// Load messages from a Hermes JSONL transcript file.
pub fn load_messages(path: &Path) -> Result<SessionMessageBatch, String> {
    load_messages_cancellable(path, &|| false)
}

pub(crate) fn load_messages_cancellable(
    path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<SessionMessageBatch, String> {
    let mut batch = SessionMessageBatchBuilder::new();
    let status = visit_bounded_lines_cancellable_with_status(path, is_cancelled, &mut |line| {
        if line.trim().is_empty() {
            return ControlFlow::Continue(());
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return ControlFlow::Continue(()),
        };

        // Support both flat messages and nested {type:"message", message:{...}} format
        let (role_val, content_val, ts_val) =
            if value.get("type").and_then(Value::as_str) == Some("message") {
                let msg = match value.get("message") {
                    Some(m) => m,
                    None => return ControlFlow::Continue(()),
                };
                (
                    msg.get("role"),
                    msg.get("content"),
                    value.get("timestamp").or_else(|| msg.get("ts")),
                )
            } else {
                (
                    value.get("role"),
                    value.get("content"),
                    value.get("timestamp").or_else(|| value.get("ts")),
                )
            };

        let role = match role_val.and_then(Value::as_str) {
            Some(r) => r.to_string(),
            None => return ControlFlow::Continue(()),
        };

        let content = content_val.map(extract_text).unwrap_or_default();
        if content.trim().is_empty() {
            return ControlFlow::Continue(());
        }

        let ts = ts_val.and_then(parse_timestamp_to_ms);
        batch.push(SessionMessage { role, content, ts })
    })
    .map_err(|error| format!("Failed to read session file: {error}"))?
    .ok_or_else(|| "Session message preview was cancelled".to_string())?;
    if status.oversized_record_skipped {
        batch.mark_truncated();
    }

    Ok(batch.finish())
}

/// Search Hermes sessions (JSONL or SQLite) for `needle` (case-insensitive).
#[allow(dead_code)]
pub fn search_sessions(metas: &[&SessionMeta], needle: &str) -> Vec<SessionSearchHit> {
    search_sessions_cancellable(metas, needle, &|| false).unwrap_or_default()
}

pub(crate) fn search_sessions_cancellable(
    metas: &[&SessionMeta],
    needle: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionSearchHit>> {
    let mut hits = Vec::new();
    for meta in metas {
        if is_cancelled() {
            return None;
        }
        let Some(source) = meta.source_path.as_deref() else {
            continue;
        };
        let hit = if source.starts_with("sqlite:") {
            search_session_sqlite(meta, needle, is_cancelled)
        } else {
            search_session_jsonl(meta, needle, is_cancelled)
        };
        if is_cancelled() {
            return None;
        }
        if let Some(hit) = hit {
            hits.push(hit);
        }
    }
    Some(hits)
}

fn search_session_jsonl(
    meta: &SessionMeta,
    needle: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<SessionSearchHit> {
    if is_cancelled() {
        return None;
    }
    let source_path = meta.source_path.as_deref()?;
    let path = Path::new(source_path);
    let mut snippets: Vec<SearchSnippet> = Vec::new();
    const MAX_SNIPPETS: usize = 5;

    visit_bounded_lines_cancellable(path, is_cancelled, &mut |line| {
        if line.trim().is_empty() {
            return ControlFlow::Continue(());
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => return ControlFlow::Continue(()),
        };
        let (role_val, content_val) =
            if value.get("type").and_then(Value::as_str) == Some("message") {
                let msg = match value.get("message") {
                    Some(m) => m,
                    None => return ControlFlow::Continue(()),
                };
                (msg.get("role"), msg.get("content"))
            } else {
                (value.get("role"), value.get("content"))
            };
        let role = match role_val.and_then(Value::as_str) {
            Some(r) => r.to_string(),
            None => return ControlFlow::Continue(()),
        };
        let content = content_val.map(extract_text).unwrap_or_default();
        if content.trim().is_empty() {
            return ControlFlow::Continue(());
        }
        match build_snippet_cancellable(&content, needle, is_cancelled) {
            Ok(Some(snippet)) => {
                snippets.push(SearchSnippet { role, snippet });
                if snippets.len() >= MAX_SNIPPETS {
                    return ControlFlow::Break(());
                }
            }
            Ok(None) => {}
            Err(_) => return ControlFlow::Break(()),
        }
        ControlFlow::Continue(())
    })
    .ok()??;
    if snippets.is_empty() {
        return None;
    }
    Some(SessionSearchHit {
        provider_id: PROVIDER_ID.to_string(),
        session_id: meta.session_id.clone(),
        source_path: source_path.to_string(),
        snippets,
    })
}

fn search_session_sqlite(
    meta: &SessionMeta,
    needle: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<SessionSearchHit> {
    if is_cancelled() {
        return None;
    }
    let source_path = meta.source_path.as_deref()?;
    let (db_path, session_id) = parse_sqlite_source(source_path)?;
    let conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let snippets = with_sqlite_cancellation(&conn, is_cancelled, || {
        // Fetch all messages for the session without a LIKE prefilter, because
        // SQLite's LIKE is case-insensitive only for ASCII and would miss Unicode
        // matches (e.g. "Éclair" vs "éclair"). The cancellable linear matcher
        // below handles Unicode lowercase matching without a second lowercase copy.
        let mut stmt = conn
            .prepare(
                "SELECT role, content FROM messages
                 WHERE session_id = ?1 AND length(CAST(content AS BLOB)) <= ?2
                 ORDER BY created_at ASC",
            )
            .ok()?;
        let rows = stmt
            .query_map(
                rusqlite::params![session_id.as_str(), MAX_METADATA_FILE_BYTES as i64],
                |row| {
                    let role: String = row.get(0)?;
                    let content: String = row.get(1)?;
                    Ok((role, content))
                },
            )
            .ok()?;
        let mut snippets: Vec<SearchSnippet> = Vec::new();
        const MAX_SNIPPETS: usize = 5;
        for row in rows.flatten() {
            if is_cancelled() {
                return None;
            }
            let (role, content) = row;
            if content.trim().is_empty() {
                continue;
            }
            if let Some(snippet) = build_snippet_cancellable(&content, needle, is_cancelled).ok()? {
                snippets.push(SearchSnippet { role, snippet });
                if snippets.len() >= MAX_SNIPPETS {
                    break;
                }
            }
        }
        Some(snippets)
    })?;
    if is_cancelled() {
        return None;
    }
    if snippets.is_empty() {
        return None;
    }
    Some(SessionSearchHit {
        provider_id: PROVIDER_ID.to_string(),
        session_id: meta.session_id.clone(),
        source_path: source_path.to_string(),
        snippets,
    })
}

/// Delete a Hermes JSONL session file.
pub fn delete_session(_root: &Path, path: &Path, _session_id: &str) -> Result<bool, String> {
    std::fs::remove_file(path).map_err(|e| {
        format!(
            "Failed to delete Hermes session file {}: {e}",
            path.display()
        )
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn parse_sqlite_source_valid() {
        let (path, id) = parse_sqlite_source("sqlite:/home/user/.hermes/state.db#session-123")
            .expect("should parse");
        assert_eq!(path, PathBuf::from("/home/user/.hermes/state.db"));
        assert_eq!(id, "session-123");
    }

    #[test]
    fn parse_sqlite_source_invalid() {
        assert!(parse_sqlite_source("not-sqlite").is_none());
        assert!(parse_sqlite_source("sqlite:").is_none());
        assert!(parse_sqlite_source("sqlite:/path#").is_none());
    }

    #[test]
    fn sqlite_stream_plan_bounds_metadata_and_omits_transcript_columns() {
        let conn = Connection::open_in_memory().expect("database");
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT NOT NULL,
                title TEXT,
                cwd TEXT,
                started_at TEXT,
                ended_at TEXT,
                transcript BLOB
             );",
        )
        .expect("schema");
        conn.execute(
            "INSERT INTO sessions
             (id, title, cwd, started_at, ended_at, transcript)
             VALUES (?1, ?2, ?3, ?4, ?5, zeroblob(16777216))",
            rusqlite::params![
                "session-1",
                "x".repeat(MAX_METADATA_LINE_BYTES + 1),
                "/tmp/project",
                "2026-01-01T00:00:00Z",
                "2026-01-01T00:01:00Z"
            ],
        )
        .expect("insert");

        let plan = HermesStreamPlan::discover(&conn).expect("plan");
        assert!(!plan.select_list.contains("transcript"));
        let mut rows = Vec::new();
        let mut stats = cache::StreamScanStats::default();
        stream_sqlite_sessions(
            &conn,
            &plan,
            &mut |meta| {
                rows.push(meta);
                ControlFlow::Continue(())
            },
            &|| false,
            &mut stats,
        )
        .expect("stream");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "session-1");
        assert!(rows[0].title.is_none(), "oversized title must not decode");
        assert_eq!(rows[0].project_dir.as_deref(), Some("/tmp/project"));
    }

    #[test]
    fn sqlite_stream_plan_rejects_schema_without_session_id() {
        let conn = Connection::open_in_memory().expect("database");
        conn.execute_batch("CREATE TABLE sessions (title TEXT);")
            .expect("schema");
        assert!(matches!(
            HermesStreamPlan::discover(&conn),
            Err(cache::StreamScanStop::Incomplete)
        ));
    }

    #[test]
    fn parse_jsonl_session_extracts_metadata() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("test-session.jsonl");
        let mut f = File::create(&path).expect("create");
        writeln!(
            f,
            r#"{{"type":"session","id":"s1","title":"My Session","cwd":"/home/user/project"}}"#
        )
        .unwrap();
        writeln!(f, r#"{{"type":"message","message":{{"role":"user","content":"Hello world"}},"timestamp":"2026-01-01T00:00:00Z"}}"#).unwrap();
        writeln!(f, r#"{{"type":"message","message":{{"role":"assistant","content":"Hi there"}},"timestamp":"2026-01-01T00:01:00Z"}}"#).unwrap();
        f.flush().unwrap();

        let meta = parse_jsonl_session(&path).expect("should parse");
        assert_eq!(meta.session_id, "s1");
        assert_eq!(meta.title.as_deref(), Some("My Session"));
        assert_eq!(meta.project_dir.as_deref(), Some("/home/user/project"));
        assert!(meta.created_at.is_some());
        assert!(meta.last_active_at.is_some());
    }

    #[test]
    fn parse_jsonl_session_fallback_to_filename() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("my-session.jsonl");
        let mut f = File::create(&path).expect("create");
        writeln!(f, r#"{{"role":"user","content":"Hello","ts":1700000000}}"#).unwrap();
        f.flush().unwrap();

        let meta = parse_jsonl_session(&path).expect("should parse");
        assert_eq!(meta.session_id, "my-session");
        assert!(meta.title.is_some()); // Falls back to first user message
    }

    #[test]
    fn load_messages_flat_format() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let mut f = File::create(&path).expect("create");
        writeln!(
            f,
            r#"{{"role":"user","content":"What is Rust?","ts":1700000000}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"role":"assistant","content":"A systems programming language.","ts":1700000001}}"#
        )
        .unwrap();
        f.flush().unwrap();

        let msgs = load_messages(&path).expect("should load");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
    }

    #[test]
    fn load_messages_nested_format() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let mut f = File::create(&path).expect("create");
        writeln!(f, r#"{{"type":"session","id":"s1"}}"#).unwrap();
        writeln!(f, r#"{{"type":"message","message":{{"role":"user","content":"Hello"}},"timestamp":"2026-01-01T00:00:00Z"}}"#).unwrap();
        writeln!(f, r#"{{"type":"message","message":{{"role":"assistant","content":"Hi"}},"timestamp":"2026-01-01T00:01:00Z"}}"#).unwrap();
        f.flush().unwrap();

        let msgs = load_messages(&path).expect("should load");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert!(msgs[0].ts.is_some());
    }

    #[test]
    fn delete_session_removes_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        File::create(&path).expect("create");
        assert!(path.exists());

        delete_session(dir.path(), &path, "session").expect("should delete");
        assert!(!path.exists());
    }
}
