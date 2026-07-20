use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::Value;

use crate::session_manager::cache::{self, FileScanTarget};
use crate::session_manager::scan_cache_store::ScanCacheStore;
use crate::session_manager::{
    append_utf8_bounded, truncate_string_utf8, SearchSnippet, SessionMessage, SessionMessageBatch,
    SessionMessageBatchBuilder, SessionMeta, SessionSearchHit,
    SESSION_MESSAGE_PREVIEW_MAX_MESSAGES, SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES,
    SESSION_MESSAGE_PREVIEW_MAX_ROLE_BYTES,
};

use super::utils::{
    build_snippet_cancellable, file_modified_ms, parse_timestamp_to_ms, path_basename,
    read_prefix_cancellable, read_to_string_cancellable, truncate_summary,
    with_sqlite_cancellation, MAX_METADATA_FILE_BYTES, MAX_METADATA_LINE_BYTES,
};

const PROVIDER_ID: &str = "opencode";

/// Return the OpenCode base directory (`$XDG_DATA_HOME/opencode`).
///
/// Respects `XDG_DATA_HOME` on all platforms; falls back to
/// `~/.local/share/opencode/`.
pub(crate) fn get_opencode_base_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("opencode");
        }
    }
    dirs::home_dir()
        .map(|h| h.join(".local/share/opencode"))
        .unwrap_or_else(|| PathBuf::from(".local/share/opencode"))
}

/// Return the OpenCode JSON storage directory (legacy flat-file layout).
pub(crate) fn get_opencode_data_dir() -> PathBuf {
    get_opencode_base_dir().join("storage")
}

fn get_opencode_db_path() -> PathBuf {
    get_opencode_base_dir().join("opencode.db")
}

/// Scan sessions from both the legacy JSON files and the newer SQLite database,
/// merging results with SQLite taking precedence on ID conflicts.
pub fn scan_sessions() -> Vec<SessionMeta> {
    merge_json_sqlite(scan_sessions_json(), scan_sessions_sqlite())
}

/// Cache-aware scan: the legacy JSON storage goes through the persistent file
/// cache, while the SQLite database is always queried fresh (a single query, not
/// worth caching). Results are merged with SQLite taking precedence — identical
/// semantics to `scan_sessions`.
pub(crate) fn scan_sessions_cached(store: &ScanCacheStore, force: bool) -> Vec<SessionMeta> {
    let storage = get_opencode_data_dir();
    let storage_for_parse = storage.clone();
    let json_sessions = cache::scan_provider_cached(
        store,
        PROVIDER_ID,
        scan_targets(&storage),
        force,
        move |path| parse_session(&storage_for_parse, path),
        // 只缓存"summary 不依赖旁路 part/ 文件"的会话，判定见 is_cacheable。
        is_cacheable,
    );
    merge_json_sqlite(json_sessions, scan_sessions_sqlite())
}

pub(crate) fn scan_sessions_progressive(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(&SessionMeta),
) -> Vec<SessionMeta> {
    let storage = get_opencode_data_dir();
    let storage_for_parse = storage.clone();
    let targets = scan_targets(&storage);
    let json_sessions = match store {
        Some(store) => cache::scan_provider_cached_progressive(
            store,
            PROVIDER_ID,
            targets,
            force,
            move |path| parse_session(&storage_for_parse, path),
            is_cacheable,
            &mut *on_session,
        ),
        None => cache::scan_provider_uncached_progressive(
            targets,
            move |path| parse_session(&storage_for_parse, path),
            &mut *on_session,
        ),
    };

    // SQLite work remains entirely on the detached session worker. Surface its
    // rows after the query so SQLite-only installations still receive a bounded
    // preview before the final cross-provider merge/sort.
    let sqlite_sessions = scan_sessions_sqlite();
    for session in &sqlite_sessions {
        on_session(session);
    }
    merge_json_sqlite(json_sessions, sqlite_sessions)
}

pub(crate) fn scan_sessions_progressive_cancellable(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(&SessionMeta),
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionMeta>> {
    let storage = get_opencode_data_dir();
    let storage_for_parse = storage.clone();
    let targets = scan_targets_cancellable(&storage, is_cancelled)?;
    let json_sessions = match store {
        Some(store) => cache::scan_provider_cached_progressive_cancellable(
            store,
            PROVIDER_ID,
            targets,
            force,
            move |path| parse_session(&storage_for_parse, path),
            is_cacheable,
            &mut *on_session,
            is_cancelled,
        )?,
        None => cache::scan_provider_uncached_progressive_cancellable(
            targets,
            move |path| parse_session(&storage_for_parse, path),
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
        merge_json_sqlite_cancellable(json_sessions, sqlite_sessions, is_cancelled)
    }
}

pub(crate) fn stream_sessions_cancellable(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(SessionMeta) -> ControlFlow<()>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<cache::StreamScanStats, cache::StreamScanStop> {
    let storage = get_opencode_data_dir();
    let message_root = storage.join("message");
    let db = open_stream_database()?;
    let mut stats = cache::StreamScanStats::default();
    if let Some(conn) = db.as_ref() {
        // SQLite is authoritative and usually the fastest source. Surface it
        // before walking legacy files so cold first paint is independent of a
        // potentially huge legacy tree or stale-cache cleanup.
        stream_sqlite_sessions(conn, on_session, is_cancelled, &mut stats)?;
    }

    let storage_for_parse = storage.clone();
    let mut duplicates = 0usize;
    let duplicate_lookup_failed = std::cell::Cell::new(false);
    let db_display = get_opencode_db_path().display().to_string();
    let mut json_sink = |meta: SessionMeta| match emit_json_with_sqlite_precedence(
        db.as_ref(),
        &db_display,
        meta,
        on_session,
        &mut duplicates,
    ) {
        Ok(flow) => flow,
        Err(()) => {
            duplicate_lookup_failed.set(true);
            ControlFlow::Break(())
        }
    };
    let file_result = cache::stream_file_provider_cancellable(
        store,
        PROVIDER_ID,
        force,
        move |path| parse_session_cancellable(&storage_for_parse, path, is_cancelled),
        is_cacheable,
        move |path| fingerprint_target(&message_root, path),
        {
            let session_root = storage.join("session");
            move |on_target, cancel| {
                let message_root = storage.join("message");
                let mut decorate = |mut target: FileScanTarget| {
                    if let Some(stem) = target
                        .path
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .map(str::to_owned)
                    {
                        let sibling = message_root.join(stem);
                        cache::mix_sibling_into_fingerprint_strict(&mut target, &sibling).map_err(
                            |error| {
                                log::warn!(
                                    "authoritative OpenCode message fingerprint failed at {}: {error}",
                                    sibling.display()
                                );
                                cache::StreamScanStop::Incomplete
                            },
                        )?;
                    }
                    on_target(target)
                };
                cache::visit_targets_recursive_cancellable(
                    &session_root,
                    "json",
                    &mut decorate,
                    cancel,
                )
            }
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

fn fingerprint_target(message_root: &Path, path: &Path) -> Option<FileScanTarget> {
    let mut target = cache::stat_target_strict(path).ok().flatten()?;
    if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
        cache::mix_sibling_into_fingerprint_strict(&mut target, &message_root.join(stem)).ok()?;
    }
    Some(target)
}

fn open_stream_database() -> Result<Option<Connection>, cache::StreamScanStop> {
    let path = get_opencode_db_path();
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(cache::StreamScanStop::Incomplete),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            log::warn!(
                "authoritative OpenCode database stat failed at {}: {error}",
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
            "authoritative OpenCode database open failed at {}: {error}",
            path.display()
        );
        cache::StreamScanStop::Incomplete
    })?;
    let has_session = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='session')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| {
            log::warn!("authoritative OpenCode schema inspection failed: {error}");
            cache::StreamScanStop::Incomplete
        })?;
    if has_session {
        Ok(Some(conn))
    } else {
        Ok(None)
    }
}

fn sqlite_stream_decodable_contains(
    conn: &Connection,
    db_display: &str,
    session_id: &str,
) -> Result<bool, ()> {
    let sql = format!(
        "SELECT {} FROM session WHERE id = ?1 LIMIT 1",
        sqlite_metadata_select_list()
    );
    conn.query_row(&sql, [session_id], |row| {
        decode_sqlite_meta(row, db_display)
    })
    .optional()
    .map(|meta| meta.is_some())
    .map_err(|error| {
        log::warn!("authoritative OpenCode duplicate decode failed for {session_id}: {error}");
    })
}

fn sqlite_metadata_select_list() -> String {
    let limit = MAX_METADATA_LINE_BYTES;
    format!(
        "CASE WHEN length(CAST(id AS BLOB)) <= {limit}
              THEN id ELSE printf('oversized-rowid-%lld', rowid) END,
         CASE WHEN length(CAST(title AS BLOB)) <= {limit} THEN title ELSE '' END,
         CASE WHEN length(CAST(directory AS BLOB)) <= {limit} THEN directory ELSE '' END,
         time_created, time_updated"
    )
}

fn emit_json_with_sqlite_precedence(
    conn: Option<&Connection>,
    db_display: &str,
    meta: SessionMeta,
    on_session: &mut dyn FnMut(SessionMeta) -> ControlFlow<()>,
    duplicates: &mut usize,
) -> Result<ControlFlow<()>, ()> {
    let duplicate = match conn {
        Some(conn) => sqlite_stream_decodable_contains(conn, db_display, &meta.session_id)?,
        None => false,
    };
    if duplicate {
        *duplicates = duplicates.saturating_add(1);
        Ok(ControlFlow::Continue(()))
    } else {
        Ok(on_session(meta))
    }
}

fn decode_sqlite_meta(row: &rusqlite::Row<'_>, db_display: &str) -> rusqlite::Result<SessionMeta> {
    let session_id: String = row.get(0)?;
    let title: String = row.get(1)?;
    let directory: String = row.get(2)?;
    let created: i64 = row.get(3)?;
    let updated: i64 = row.get(4)?;
    let display_title = if title.is_empty() {
        path_basename(&directory)
    } else {
        Some(title)
    };
    Ok(SessionMeta {
        provider_id: PROVIDER_ID.to_string(),
        session_id: session_id.clone(),
        title: display_title.clone(),
        summary: display_title,
        project_dir: (!directory.is_empty()).then_some(directory),
        created_at: Some(created),
        last_active_at: Some(updated),
        source_path: Some(format!("sqlite:{db_display}:{session_id}")),
        resume_command: Some(format!("opencode session resume {session_id}")),
    })
}

fn stream_sqlite_sessions(
    conn: &Connection,
    on_session: &mut dyn FnMut(SessionMeta) -> ControlFlow<()>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    stats: &mut cache::StreamScanStats,
) -> Result<(), cache::StreamScanStop> {
    if is_cancelled() {
        return Err(cache::StreamScanStop::Cancelled);
    }
    // The page-manifest builder supplies the final total ordering, so omitting
    // ORDER BY here lets SQLite step rows immediately instead of materializing
    // a provider-wide sort before the first callback.
    let sql = format!("SELECT {} FROM session", sqlite_metadata_select_list());
    let mut stmt = conn.prepare(&sql).map_err(|error| {
        log::warn!("authoritative OpenCode session query prepare failed: {error}");
        cache::StreamScanStop::Incomplete
    })?;
    let db_display = get_opencode_db_path().display().to_string();
    let rows = stmt
        .query_map([], |row| decode_sqlite_meta(row, &db_display))
        .map_err(|error| {
            log::warn!("authoritative OpenCode session query failed: {error}");
            cache::StreamScanStop::Incomplete
        })?;
    for row in rows {
        if is_cancelled() {
            return Err(cache::StreamScanStop::Cancelled);
        }
        let meta = row.map_err(|error| {
            log::warn!("authoritative OpenCode session row decode failed: {error}");
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

/// OpenCode 的 sidecar 扫描缓存 cacheable 谓词：仅缓存"summary 不依赖旁路
/// part/ 文件"的会话。
///
/// 不能只用 `title.is_some()`：`parse_session` 会把无显式 title 的会话用
/// directory basename 回填 `display_title`（见其内注释），于是 `meta.title`
/// 几乎总是 `Some`，无法区分"summary 派生自 part/ 正文文件"的会话——它们的
/// summary 晚到/变化不改变 session JSON 与 message 目录 stat，缓存会长期返回
/// 旧 summary。
///
/// 改用 `parse_session` 的构造不变量：**有显式 title 时 `summary == title`
/// （同一字符串 clone）；无显式 title 时 title 来自目录名、summary 来自 part
/// 文本，且 `parse_session` 会把恰好等于 title 的派生 summary 丢弃为 None**。
/// 因此 `summary == Some(title)` **当且仅当**显式 title——不再依赖"两者不同源
/// 必不相等"的概率论证，而是由构造严格保证（巧合窗口已在 `parse_session` 消除）。
/// 故本谓词恰好识别"无 part 文件依赖"的会话——多数会话（有显式 title）可缓存，
/// 其余每轮重新解析，代价小且保证正确。此谓词与 `parse_session` 的 summary 构造
/// 强绑定：改动那里必须同步审视这里（不变量已由本文件单元测试固化）。
fn is_cacheable(meta: &SessionMeta) -> bool {
    meta.title.is_some() && meta.summary == meta.title
}

/// Merge legacy-JSON and SQLite sessions, keeping the SQLite row when the same
/// `session_id` exists in both.
fn merge_json_sqlite(
    json_sessions: Vec<SessionMeta>,
    sqlite_sessions: Vec<SessionMeta>,
) -> Vec<SessionMeta> {
    if sqlite_sessions.is_empty() {
        return json_sessions;
    }
    if json_sessions.is_empty() {
        return sqlite_sessions;
    }

    let sqlite_ids: std::collections::HashSet<String> = sqlite_sessions
        .iter()
        .map(|s| s.session_id.clone())
        .collect();

    let mut merged = sqlite_sessions;
    for s in json_sessions {
        if !sqlite_ids.contains(&s.session_id) {
            merged.push(s);
        }
    }
    merged
}

fn merge_json_sqlite_cancellable(
    json_sessions: Vec<SessionMeta>,
    sqlite_sessions: Vec<SessionMeta>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionMeta>> {
    if is_cancelled() {
        return None;
    }
    if sqlite_sessions.is_empty() {
        return Some(json_sessions);
    }
    if json_sessions.is_empty() {
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
    for session in json_sessions {
        if is_cancelled() {
            return None;
        }
        if !sqlite_ids.contains(&session.session_id) {
            merged.push(session);
        }
    }
    Some(merged)
}

fn scan_targets(storage: &Path) -> Vec<FileScanTarget> {
    scan_targets_cancellable(storage, &|| false).expect("non-cancellable target scan cannot stop")
}

fn scan_targets_cancellable(
    storage: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<FileScanTarget>> {
    let mut targets = Vec::new();
    if !cache::collect_targets_recursive_cancellable(
        &storage.join("session"),
        "json",
        &mut targets,
        is_cancelled,
    ) {
        return None;
    }
    // message 目录指纹只服务于**有 title** 的会话行的常规失效（例如会话新增
    // 消息 → 目录 mtime 变 → 缓存重解析拿到更新的 last_active）。无 title 的
    // 行由上面的 cacheable 判定直接不缓存、每轮重解析，不依赖这里的指纹；两
    // 者分工互补。有 title 的会话混入此指纹无害，故保留。
    let message_root = storage.join("message");
    for target in &mut targets {
        if is_cancelled() {
            return None;
        }
        if let Some(stem) = target.path.file_stem().and_then(|s| s.to_str()) {
            cache::mix_sibling_into_fingerprint(target, &message_root.join(stem));
        }
    }
    Some(targets)
}

fn scan_sessions_json() -> Vec<SessionMeta> {
    let storage = get_opencode_data_dir();
    let session_dir = storage.join("session");
    if !session_dir.exists() {
        return Vec::new();
    }

    let mut json_files = Vec::new();
    collect_json_files(&session_dir, &mut json_files);

    let mut sessions = Vec::new();
    for path in json_files {
        if let Some(meta) = parse_session(&storage, &path) {
            sessions.push(meta);
        }
    }
    sessions
}

/// Parse a SQLite source reference in the format `sqlite:<db_path>:<session_id>`.
///
/// Uses `rfind(":ses_")` to split the path from the session ID because the
/// db path itself may contain colons (e.g. `C:\Users\...` on Windows).
/// This relies on the OpenCode convention that session IDs start with `ses_`.
fn parse_sqlite_source(source: &str) -> Option<(PathBuf, String)> {
    let rest = source.strip_prefix("sqlite:")?;
    let sep = rest.rfind(":ses_")?;
    let db_path = PathBuf::from(&rest[..sep]);
    let session_id = rest[sep + 1..].to_string();
    Some((db_path, session_id))
}

fn scan_sessions_sqlite() -> Vec<SessionMeta> {
    scan_sessions_sqlite_impl(&|| false, true)
        .expect("non-cancellable SQLite session scan cannot stop")
}

fn scan_sessions_sqlite_cancellable(
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionMeta>> {
    scan_sessions_sqlite_impl(is_cancelled, false)
}

fn scan_sessions_sqlite_impl(
    is_cancelled: &(dyn Fn() -> bool + Sync),
    ordered: bool,
) -> Option<Vec<SessionMeta>> {
    if is_cancelled() {
        return None;
    }
    let db_path = get_opencode_db_path();
    if !db_path.exists() {
        return Some(Vec::new());
    }

    let conn = match Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(_) => return Some(Vec::new()),
    };

    let query = if ordered {
        format!(
            "SELECT {} FROM session ORDER BY time_updated DESC",
            sqlite_metadata_select_list()
        )
    } else {
        // Search does not depend on metadata order. Omitting ORDER BY lets row
        // stepping observe cancellation without first sorting the whole table.
        format!("SELECT {} FROM session", sqlite_metadata_select_list())
    };
    let mut stmt = match conn.prepare(&query) {
        Ok(s) => s,
        Err(_) => return Some(Vec::new()),
    };

    let db_display = db_path.display().to_string();

    let iter = match stmt.query_map([], |row| decode_sqlite_meta(row, &db_display)) {
        Ok(rows) => rows,
        Err(_) => return Some(Vec::new()),
    };

    let mut sessions = Vec::new();
    for meta in iter.flatten() {
        if is_cancelled() {
            return None;
        }
        sessions.push(meta);
    }
    if is_cancelled() {
        None
    } else {
        Some(sessions)
    }
}

pub fn load_messages(path: &Path) -> Result<SessionMessageBatch, String> {
    load_messages_cancellable(path, &|| false)
}

#[derive(Debug)]
struct OpenCodeMessageHeader {
    created_ts: i64,
    message_id: String,
    role: String,
}

pub(crate) fn load_messages_cancellable(
    path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<SessionMessageBatch, String> {
    // `path` is the message directory: storage/message/{sessionID}/
    if !path.is_dir() {
        return Err(format!("Message directory not found: {}", path.display()));
    }

    let storage = path
        .parent()
        .and_then(|p| p.parent())
        .ok_or_else(|| "Cannot determine storage root from message path".to_string())?;
    let retained_limit = SESSION_MESSAGE_PREVIEW_MAX_MESSAGES + 1;
    let mut headers = Vec::with_capacity(retained_limit);
    let mut valid_headers = 0usize;
    let mut truncated = false;
    let visited = visit_json_files_streaming(path, is_cancelled, &mut |msg_path| {
        let data = match read_to_string_cancellable(msg_path, is_cancelled) {
            Ok(Some(data)) => data,
            Ok(None) => return ControlFlow::Break(()),
            Err(_) => {
                truncated = true;
                return ControlFlow::Continue(());
            }
        };
        let value: Value = match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(_) => return ControlFlow::Continue(()),
        };
        let Some(message_id) = value.get("id").and_then(Value::as_str) else {
            return ControlFlow::Continue(());
        };
        if message_id.len() > SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES {
            truncated = true;
            return ControlFlow::Continue(());
        }
        let mut role = value
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        if truncate_string_utf8(&mut role, SESSION_MESSAGE_PREVIEW_MAX_ROLE_BYTES) {
            truncated = true;
        }
        let candidate = OpenCodeMessageHeader {
            created_ts: value
                .get("time")
                .and_then(|time| time.get("created"))
                .and_then(parse_timestamp_to_ms)
                .unwrap_or(0),
            message_id: message_id.to_string(),
            role,
        };
        valid_headers = valid_headers.saturating_add(1);
        retain_recent_opencode_header(&mut headers, candidate, retained_limit);
        ControlFlow::Continue(())
    });
    if visited.is_none() || is_cancelled() {
        return Err("Session message preview was cancelled".to_string());
    }

    headers.sort_by(|left, right| {
        (left.created_ts, left.message_id.as_str())
            .cmp(&(right.created_ts, right.message_id.as_str()))
    });
    if valid_headers > SESSION_MESSAGE_PREVIEW_MAX_MESSAGES {
        truncated = true;
    }
    if headers.len() > SESSION_MESSAGE_PREVIEW_MAX_MESSAGES {
        let remove = headers.len() - SESSION_MESSAGE_PREVIEW_MAX_MESSAGES;
        headers.drain(..remove);
    }

    let mut batch = SessionMessageBatchBuilder::new();
    if truncated {
        batch.mark_truncated();
    }
    // Apply the aggregate byte cap from newest to oldest, then restore the
    // retained window to chronological display order.
    for header in headers.into_iter().rev() {
        if is_cancelled() {
            return Err("Session message preview was cancelled".to_string());
        }
        let (content, content_truncated) = load_opencode_part_preview(
            &storage.join("part").join(&header.message_id),
            is_cancelled,
        )?;
        if content_truncated {
            batch.mark_truncated();
        }
        if content.trim().is_empty() {
            continue;
        }
        if batch
            .push(SessionMessage {
                role: header.role,
                content,
                ts: (header.created_ts > 0).then_some(header.created_ts),
            })
            .is_break()
        {
            break;
        }
    }
    let mut batch = batch.finish();
    batch.messages.reverse();
    Ok(batch)
}

/// Load messages from the OpenCode SQLite database for a given source reference.
/// Selects the latest bounded window and reconstructs each message lazily.
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
    .map_err(|e| format!("Failed to open OpenCode database: {e}"))?;

    with_sqlite_cancellation(&conn, is_cancelled, || {
        load_messages_sqlite_from_connection(&conn, &session_id, is_cancelled)
    })
}

fn load_messages_sqlite_from_connection(
    conn: &Connection,
    session_id: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<SessionMessageBatch, String> {
    let mut msg_stmt = conn
        .prepare(&format!(
            "SELECT
                CASE WHEN length(CAST(id AS BLOB)) <= {SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES}
                     THEN CAST(substr(CAST(id AS BLOB), 1, {SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES}) AS TEXT)
                     ELSE NULL END,
                length(CAST(id AS BLOB)),
                time_created,
                CASE WHEN length(CAST(data AS BLOB)) <= {MAX_METADATA_FILE_BYTES}
                     THEN CAST(substr(CAST(data AS BLOB), 1, {MAX_METADATA_FILE_BYTES}) AS TEXT)
                     ELSE NULL END,
                length(CAST(data AS BLOB))
             FROM message
             WHERE session_id = ?1
             ORDER BY time_created DESC
             LIMIT {}",
            SESSION_MESSAGE_PREVIEW_MAX_MESSAGES + 1
        ))
        .map_err(|e| format!("Failed to prepare message query: {e}"))?;

    let msg_rows = msg_stmt
        .query_map([session_id], |row| {
            let id: Option<String> = row.get(0)?;
            let id_bytes: Option<i64> = row.get(1)?;
            let ts: i64 = row.get(2)?;
            let data: Option<String> = row.get(3)?;
            let data_bytes: Option<i64> = row.get(4)?;
            Ok((id, id_bytes, ts, data, data_bytes))
        })
        .map_err(|e| format!("Failed to query messages: {e}"))?;

    let mut part_stmt = conn
        .prepare(&format!(
            "SELECT
                CASE WHEN length(CAST(data AS BLOB)) <= {MAX_METADATA_FILE_BYTES}
                     THEN CAST(substr(CAST(data AS BLOB), 1, {MAX_METADATA_FILE_BYTES}) AS TEXT)
                     ELSE NULL END,
                length(CAST(data AS BLOB))
             FROM part
             WHERE session_id = ?1 AND message_id = ?2
             ORDER BY time_created ASC
             LIMIT {}",
            SESSION_MESSAGE_PREVIEW_MAX_MESSAGES + 1
        ))
        .map_err(|e| format!("Failed to prepare part query: {e}"))?;

    let mut newest = Vec::with_capacity(SESSION_MESSAGE_PREVIEW_MAX_MESSAGES + 1);
    let mut truncated = false;
    for row in msg_rows {
        if is_cancelled() {
            return Err("Session message preview was cancelled".to_string());
        }
        let (id, id_bytes, ts, data, data_bytes) =
            row.map_err(|error| format!("Failed to decode message row: {error}"))?;
        let Some(message_id) = id else {
            truncated = true;
            continue;
        };
        truncated |= id_bytes
            .is_some_and(|bytes| bytes > SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES as i64)
            || data_bytes.is_some_and(|bytes| bytes > MAX_METADATA_FILE_BYTES as i64);
        let mut role = data
            .as_deref()
            .and_then(|value| serde_json::from_str::<Value>(value).ok())
            .and_then(|value| value.get("role").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_string());
        if truncate_string_utf8(&mut role, SESSION_MESSAGE_PREVIEW_MAX_ROLE_BYTES) {
            truncated = true;
        }
        newest.push(OpenCodeMessageHeader {
            created_ts: ts,
            message_id,
            role,
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
    // `newest` is DESC. Apply the aggregate byte cap at the newest end before
    // restoring chronological display order.
    for header in newest {
        if is_cancelled() {
            return Err("Session message preview was cancelled".to_string());
        }
        let (content, content_truncated) = load_opencode_sqlite_parts_preview(
            &mut part_stmt,
            session_id,
            &header.message_id,
            is_cancelled,
        )?;
        if content_truncated {
            batch.mark_truncated();
        }
        if content.trim().is_empty() {
            continue;
        }
        if batch
            .push(SessionMessage {
                role: header.role,
                content,
                ts: Some(header.created_ts),
            })
            .is_break()
        {
            break;
        }
    }

    let mut batch = batch.finish();
    batch.messages.reverse();
    Ok(batch)
}

fn retain_recent_opencode_header(
    headers: &mut Vec<OpenCodeMessageHeader>,
    candidate: OpenCodeMessageHeader,
    limit: usize,
) {
    if headers.len() < limit {
        headers.push(candidate);
        return;
    }
    let Some((oldest, _)) = headers.iter().enumerate().min_by(|(_, left), (_, right)| {
        (left.created_ts, left.message_id.as_str())
            .cmp(&(right.created_ts, right.message_id.as_str()))
    }) else {
        return;
    };
    if (candidate.created_ts, candidate.message_id.as_str())
        > (
            headers[oldest].created_ts,
            headers[oldest].message_id.as_str(),
        )
    {
        headers[oldest] = candidate;
    }
}

fn load_opencode_part_preview(
    part_dir: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(String, bool), String> {
    let mut content = String::new();
    let mut truncated = false;
    let mut visited_parts = 0usize;
    let visited = visit_json_files_streaming(part_dir, is_cancelled, &mut |part_path| {
        if visited_parts >= SESSION_MESSAGE_PREVIEW_MAX_MESSAGES {
            truncated = true;
            return ControlFlow::Break(());
        }
        visited_parts += 1;
        let data = match read_to_string_cancellable(part_path, is_cancelled) {
            Ok(Some(data)) => data,
            Ok(None) => return ControlFlow::Break(()),
            Err(_) => {
                truncated = true;
                return ControlFlow::Continue(());
            }
        };
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            return ControlFlow::Continue(());
        };
        let Some(part) = extract_part_text(&value) else {
            return ControlFlow::Continue(());
        };
        if !content.is_empty()
            && append_utf8_bounded(
                &mut content,
                "\n",
                SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES,
            )
        {
            truncated = true;
            return ControlFlow::Break(());
        }
        if append_utf8_bounded(
            &mut content,
            &part,
            SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES,
        ) || content.len() >= SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES
        {
            truncated = true;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    if visited.is_none() || is_cancelled() {
        Err("Session message preview was cancelled".to_string())
    } else {
        Ok((content, truncated))
    }
}

fn load_opencode_sqlite_parts_preview(
    stmt: &mut rusqlite::Statement<'_>,
    session_id: &str,
    message_id: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(String, bool), String> {
    let rows = stmt
        .query_map(rusqlite::params![session_id, message_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<i64>>(1)?,
            ))
        })
        .map_err(|error| format!("Failed to query message parts: {error}"))?;
    let mut content = String::new();
    let mut truncated = false;
    for (index, row) in rows.enumerate() {
        if is_cancelled() {
            return Err("Session message preview was cancelled".to_string());
        }
        if index >= SESSION_MESSAGE_PREVIEW_MAX_MESSAGES {
            truncated = true;
            break;
        }
        let (data, data_bytes) =
            row.map_err(|error| format!("Failed to decode message part: {error}"))?;
        let Some(data) = data else {
            truncated = true;
            continue;
        };
        truncated |= data_bytes.is_some_and(|bytes| bytes > MAX_METADATA_FILE_BYTES as i64);
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let Some(part) = extract_part_text(&value) else {
            continue;
        };
        if !content.is_empty()
            && append_utf8_bounded(
                &mut content,
                "\n",
                SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES,
            )
        {
            truncated = true;
            break;
        }
        if append_utf8_bounded(
            &mut content,
            &part,
            SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES,
        ) || content.len() >= SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES
        {
            truncated = true;
            break;
        }
    }
    Ok((content, truncated))
}

/// Search OpenCode sessions (file-based or SQLite) for `needle` (case-insensitive).
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
            search_session_files(meta, needle, is_cancelled)
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

const MAX_SEARCH_SNIPPETS: usize = 5;

fn search_session_files(
    meta: &SessionMeta,
    needle: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<SessionSearchHit> {
    if is_cancelled() {
        return None;
    }
    let source_path = meta.source_path.as_deref()?;
    let msg_dir = Path::new(source_path);
    if !msg_dir.is_dir() {
        return None;
    }
    let storage = msg_dir.parent().and_then(|p| p.parent())?;
    let mut entries: Vec<(i64, SearchSnippet)> = Vec::new();

    let _ = visit_json_files_streaming(msg_dir, is_cancelled, &mut |msg_path| {
        let data = match read_to_string_cancellable(msg_path, is_cancelled) {
            Ok(Some(data)) => data,
            Ok(None) => return ControlFlow::Break(()),
            Err(_) => return ControlFlow::Continue(()),
        };
        let value: Value = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(_) => return ControlFlow::Continue(()),
        };
        let msg_id = match value.get("id").and_then(Value::as_str) {
            Some(id) => id.to_string(),
            None => return ControlFlow::Continue(()),
        };
        let role = value
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let created_ts = value
            .get("time")
            .and_then(|t| t.get("created"))
            .and_then(parse_timestamp_to_ms)
            .unwrap_or(0);
        let part_dir = storage.join("part").join(&msg_id);
        let Some(text) = collect_parts_text_cancellable(&part_dir, is_cancelled) else {
            return ControlFlow::Break(());
        };
        if text.trim().is_empty() {
            return ControlFlow::Continue(());
        }
        match build_snippet_cancellable(&text, needle, is_cancelled) {
            Ok(Some(snippet)) => {
                entries.push((created_ts, SearchSnippet { role, snippet }));
                entries.sort_by_key(|(timestamp, _)| *timestamp);
                entries.truncate(MAX_SEARCH_SNIPPETS);
            }
            Ok(None) => {}
            Err(_) => return ControlFlow::Break(()),
        }
        ControlFlow::Continue(())
    })?;
    if is_cancelled() {
        return None;
    }
    let snippets: Vec<SearchSnippet> = entries.into_iter().map(|(_, snippet)| snippet).collect();
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
        let mut msg_stmt = conn
            .prepare(
                "SELECT id, time_created, data FROM message
                 WHERE session_id = ?1 AND length(CAST(data AS BLOB)) <= ?2
                 ORDER BY time_created ASC",
            )
            .ok()?;
        let msg_rows = msg_stmt
            .query_map(
                rusqlite::params![session_id.as_str(), MAX_METADATA_FILE_BYTES as i64],
                |row| {
                    let id: String = row.get(0)?;
                    let ts: i64 = row.get(1)?;
                    let data: String = row.get(2)?;
                    Ok((id, ts, data))
                },
            )
            .ok()?;
        let mut part_stmt = conn
            .prepare(
                "SELECT data FROM part
                 WHERE session_id = ?1 AND message_id = ?2
                   AND length(CAST(data AS BLOB)) <= ?3
                 ORDER BY time_created ASC",
            )
            .ok()?;
        let mut snippets: Vec<SearchSnippet> = Vec::new();
        for row in msg_rows.flatten() {
            if is_cancelled() {
                return None;
            }
            let (msg_id, _ts, data) = row;
            let msg_value: Value = match serde_json::from_str(&data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let role = msg_value
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let mut text = String::new();
            let part_rows = part_stmt
                .query_map(
                    rusqlite::params![
                        session_id.as_str(),
                        msg_id.as_str(),
                        MAX_METADATA_FILE_BYTES as i64
                    ],
                    |row| row.get::<_, String>(0),
                )
                .ok()?;
            for part_data in part_rows.flatten() {
                if is_cancelled() {
                    return None;
                }
                let part_value: Value = match serde_json::from_str(&part_data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(part) = extract_part_text(&part_value) {
                    if !text.is_empty() && text.len() < MAX_METADATA_FILE_BYTES {
                        text.push('\n');
                    }
                    for ch in part.chars() {
                        if text.len().saturating_add(ch.len_utf8()) > MAX_METADATA_FILE_BYTES {
                            break;
                        }
                        text.push(ch);
                    }
                    if text.len() >= MAX_METADATA_FILE_BYTES {
                        break;
                    }
                }
            }
            if text.trim().is_empty() {
                continue;
            }
            if let Some(snippet) = build_snippet_cancellable(&text, needle, is_cancelled).ok()? {
                snippets.push(SearchSnippet { role, snippet });
                if snippets.len() >= MAX_SEARCH_SNIPPETS {
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

pub fn delete_session(storage: &Path, path: &Path, session_id: &str) -> Result<bool, String> {
    if path.file_name().and_then(|name| name.to_str()) != Some(session_id) {
        return Err(format!(
            "OpenCode session path does not match session ID: expected {session_id}, found {}",
            path.display()
        ));
    }

    let mut message_files = Vec::new();
    collect_json_files(path, &mut message_files);

    let mut message_ids = Vec::new();
    for message_path in &message_files {
        let data = match std::fs::read_to_string(message_path) {
            Ok(data) => data,
            Err(_) => continue,
        };
        let value: Value = match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if let Some(message_id) = value.get("id").and_then(Value::as_str) {
            message_ids.push(message_id.to_string());
        }
    }

    for message_id in &message_ids {
        let part_dir = storage.join("part").join(message_id);
        remove_dir_all_if_exists(&part_dir).map_err(|e| {
            format!(
                "Failed to delete OpenCode part directory {}: {e}",
                part_dir.display()
            )
        })?;
    }

    let session_diff_path = storage
        .join("session_diff")
        .join(format!("{session_id}.json"));
    remove_file_if_exists(&session_diff_path).map_err(|e| {
        format!(
            "Failed to delete OpenCode session diff {}: {e}",
            session_diff_path.display()
        )
    })?;

    remove_dir_all_if_exists(path).map_err(|e| {
        format!(
            "Failed to delete OpenCode message directory {}: {e}",
            path.display()
        )
    })?;

    if let Some(session_file) = find_session_file(storage, session_id) {
        remove_file_if_exists(&session_file).map_err(|e| {
            format!(
                "Failed to delete OpenCode session file {}: {e}",
                session_file.display()
            )
        })?;
    }

    Ok(true)
}

/// Delete a session from the OpenCode SQLite database.
pub fn delete_session_sqlite(session_id: &str, source: &str) -> Result<bool, String> {
    let (db_path, ref_session_id) = parse_sqlite_source(source)
        .ok_or_else(|| format!("Invalid SQLite source reference: {source}"))?;
    let db_path = db_path
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize SQLite database path: {e}"))?;
    let expected_db_path = get_opencode_db_path()
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize expected OpenCode database path: {e}"))?;

    if ref_session_id != session_id {
        return Err(format!(
            "OpenCode SQLite session ID mismatch: expected {session_id}, found {ref_session_id}"
        ));
    }
    if db_path != expected_db_path {
        return Err("SQLite path does not match expected OpenCode database".to_string());
    }

    let conn =
        Connection::open(&db_path).map_err(|e| format!("Failed to open OpenCode database: {e}"))?;

    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("Failed to begin transaction: {e}"))?;

    tx.execute("DELETE FROM part WHERE session_id = ?1", [session_id])
        .map_err(|e| format!("Failed to delete OpenCode parts: {e}"))?;
    tx.execute("DELETE FROM message WHERE session_id = ?1", [session_id])
        .map_err(|e| format!("Failed to delete OpenCode messages: {e}"))?;

    let deleted = tx
        .execute("DELETE FROM session WHERE id = ?1", [session_id])
        .map_err(|e| format!("Failed to delete OpenCode session: {e}"))?;

    tx.commit()
        .map_err(|e| format!("Failed to commit session deletion: {e}"))?;

    Ok(deleted > 0)
}

fn parse_session(storage: &Path, path: &Path) -> Option<SessionMeta> {
    parse_session_cancellable(storage, path, &|| false)
        .ok()
        .flatten()
}

fn parse_session_cancellable(
    storage: &Path,
    path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<SessionMeta>, cache::StreamScanStop> {
    let data = match read_to_string_cancellable(path, is_cancelled) {
        Ok(Some(data)) => data,
        Ok(None) => return Err(cache::StreamScanStop::Cancelled),
        Err(error) if error.kind() == std::io::ErrorKind::FileTooLarge => {
            let prefix = read_prefix_cancellable(path, MAX_METADATA_LINE_BYTES, is_cancelled)
                .map_err(|error| {
                    log::warn!(
                        "authoritative oversized OpenCode metadata fallback failed at {}: {error}",
                        path.display()
                    );
                    cache::StreamScanStop::Incomplete
                })?
                .ok_or(cache::StreamScanStop::Cancelled)?;
            return Ok(Some(parse_session_lightweight(
                storage,
                path,
                &prefix,
                is_cancelled,
            )));
        }
        Err(error) => {
            log::warn!(
                "authoritative OpenCode metadata read failed at {}: {error}",
                path.display()
            );
            return Err(cache::StreamScanStop::Incomplete);
        }
    };
    if is_cancelled() {
        return Err(cache::StreamScanStop::Cancelled);
    }
    let value: Value = match serde_json::from_str(&data) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let Some(session_id) = value.get("id").and_then(Value::as_str).map(str::to_string) else {
        return Ok(None);
    };
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let directory = value
        .get("directory")
        .and_then(Value::as_str)
        .map(str::to_string);
    let created_at = value
        .get("time")
        .and_then(|time| time.get("created"))
        .and_then(parse_timestamp_to_ms);
    let updated_at = value
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(parse_timestamp_to_ms);
    let has_title = title.is_some();
    let display_title = title.or_else(|| directory.as_deref().and_then(path_basename));
    let summary = if has_title {
        display_title.clone()
    } else {
        match get_first_user_summary_streaming(storage, &session_id, is_cancelled) {
            Some(summary) if Some(&summary) == display_title.as_ref() => None,
            other => other,
        }
    };
    if is_cancelled() {
        return Err(cache::StreamScanStop::Cancelled);
    }
    Ok(Some(SessionMeta {
        provider_id: PROVIDER_ID.to_string(),
        session_id: session_id.clone(),
        title: display_title,
        summary,
        project_dir: directory,
        created_at,
        last_active_at: updated_at.or(created_at),
        source_path: Some(
            storage
                .join("message")
                .join(&session_id)
                .to_string_lossy()
                .into_owned(),
        ),
        resume_command: Some(format!("opencode session resume {session_id}")),
    }))
}

fn parse_session_lightweight(
    storage: &Path,
    path: &Path,
    prefix: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> SessionMeta {
    let session_id = extract_json_value(prefix, "id")
        .and_then(|value| value.as_str().map(str::to_owned))
        .or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let explicit_title = extract_json_value(prefix, "title")
        .and_then(|value| value.as_str().map(str::to_owned))
        .filter(|value| !value.is_empty());
    let directory =
        extract_json_value(prefix, "directory").and_then(|value| value.as_str().map(str::to_owned));
    let created_at =
        extract_json_value(prefix, "created").and_then(|value| parse_timestamp_to_ms(&value));
    let updated_at = extract_json_value(prefix, "updated")
        .and_then(|value| parse_timestamp_to_ms(&value))
        .or_else(|| file_modified_ms(path));
    let display_title = explicit_title
        .clone()
        .or_else(|| directory.as_deref().and_then(path_basename))
        .or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        });
    let summary = if explicit_title.is_some() {
        display_title.clone()
    } else {
        get_first_user_summary_streaming(storage, &session_id, is_cancelled)
            .filter(|summary| Some(summary) != display_title.as_ref())
    };
    SessionMeta {
        provider_id: PROVIDER_ID.to_string(),
        session_id: session_id.clone(),
        title: display_title,
        summary,
        project_dir: directory,
        created_at: created_at.or(updated_at),
        last_active_at: updated_at.or(created_at),
        source_path: Some(
            storage
                .join("message")
                .join(&session_id)
                .to_string_lossy()
                .into_owned(),
        ),
        resume_command: Some(format!("opencode session resume {session_id}")),
    }
}

fn extract_json_value(input: &str, key: &str) -> Option<Value> {
    let needle = format!("\"{key}\"");
    for (offset, _) in input.match_indices(&needle) {
        let tail = input.get(offset + needle.len()..)?;
        let value_start = tail.find(':')? + 1;
        let mut deserializer = serde_json::Deserializer::from_str(tail[value_start..].trim_start());
        if let Ok(value) = Value::deserialize(&mut deserializer) {
            return Some(value);
        }
    }
    None
}

fn get_first_user_summary_streaming(
    storage: &Path,
    session_id: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<String> {
    let mut first: Option<(i64, String)> = None;
    let message_dir = storage.join("message").join(session_id);
    let outcome = visit_json_files_streaming(&message_dir, is_cancelled, &mut |path| {
        let Some(data) = read_to_string_cancellable(path, is_cancelled)
            .ok()
            .flatten()
        else {
            return ControlFlow::Continue(());
        };
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            return ControlFlow::Continue(());
        };
        if value.get("role").and_then(Value::as_str) != Some("user") {
            return ControlFlow::Continue(());
        }
        let Some(message_id) = value.get("id").and_then(Value::as_str) else {
            return ControlFlow::Continue(());
        };
        let timestamp = value
            .get("time")
            .and_then(|time| time.get("created"))
            .and_then(parse_timestamp_to_ms)
            .unwrap_or(0);
        let candidate = (timestamp, message_id.to_string());
        if first.as_ref().is_none_or(|current| candidate < *current) {
            first = Some(candidate);
        }
        ControlFlow::Continue(())
    });
    if outcome.is_none() || is_cancelled() {
        return None;
    }
    let (_, message_id) = first?;
    let mut text = String::new();
    let part_dir = storage.join("part").join(message_id);
    let outcome = visit_json_files_streaming(&part_dir, is_cancelled, &mut |path| {
        let Some(data) = read_to_string_cancellable(path, is_cancelled)
            .ok()
            .flatten()
        else {
            return ControlFlow::Continue(());
        };
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            return ControlFlow::Continue(());
        };
        if let Some(part) = extract_part_text(&value) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.extend(
                part.chars()
                    .take(161usize.saturating_sub(text.chars().count())),
            );
        }
        if text.chars().count() >= 161 {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    if outcome.is_none() || is_cancelled() || text.trim().is_empty() {
        None
    } else {
        Some(truncate_summary(&text, 160))
    }
}

/// `None` means cancellation; `Break` propagates a successful bounded early
/// stop through every recursion level.
fn visit_json_files_streaming(
    root: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    on_file: &mut dyn FnMut(&Path) -> ControlFlow<()>,
) -> Option<ControlFlow<()>> {
    if is_cancelled() {
        return None;
    }
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Some(ControlFlow::Continue(())),
    };
    for entry in entries.flatten() {
        if is_cancelled() {
            return None;
        }
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            if visit_json_files_streaming(&path, is_cancelled, on_file)?.is_break() {
                return Some(ControlFlow::Break(()));
            }
        } else if path.extension().and_then(|value| value.to_str()) == Some("json")
            && on_file(&path).is_break()
        {
            return Some(ControlFlow::Break(()));
        }
    }
    Some(ControlFlow::Continue(()))
}

/// Collect text content from all parts in a part directory.
fn extract_part_text(part_value: &Value) -> Option<String> {
    match part_value.get("type").and_then(Value::as_str) {
        Some("text") => part_value
            .get("text")
            .and_then(Value::as_str)
            .filter(|t| !t.trim().is_empty())
            .map(|t| t.to_string()),
        Some("tool") => {
            let tool = part_value
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            Some(format!("[Tool: {tool}]"))
        }
        _ => None,
    }
}

fn collect_parts_text(part_dir: &Path) -> String {
    if !part_dir.is_dir() {
        return String::new();
    }

    let mut parts = Vec::new();
    collect_json_files(part_dir, &mut parts);

    let mut texts = Vec::new();
    for part_path in &parts {
        let data = match read_to_string_cancellable(part_path, &|| false) {
            Ok(Some(data)) => data,
            Ok(None) | Err(_) => continue,
        };
        let value: Value = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(text) = extract_part_text(&value) {
            texts.push(text);
        }
    }

    texts.join("\n")
}

fn collect_parts_text_cancellable(
    part_dir: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<String> {
    if !part_dir.is_dir() {
        return Some(String::new());
    }
    let mut text = String::new();
    let _ = visit_json_files_streaming(part_dir, is_cancelled, &mut |part_path| {
        let data = match read_to_string_cancellable(part_path, is_cancelled) {
            Ok(Some(data)) => data,
            Ok(None) => return ControlFlow::Break(()),
            Err(_) => return ControlFlow::Continue(()),
        };
        let value: Value = match serde_json::from_str(&data) {
            Ok(value) => value,
            Err(_) => return ControlFlow::Continue(()),
        };
        if let Some(part) = extract_part_text(&value) {
            if !text.is_empty() && text.len() < MAX_METADATA_FILE_BYTES {
                text.push('\n');
            }
            for ch in part.chars() {
                if text.len().saturating_add(ch.len_utf8()) > MAX_METADATA_FILE_BYTES {
                    return ControlFlow::Break(());
                }
                text.push(ch);
            }
        }
        ControlFlow::Continue(())
    })?;
    if is_cancelled() {
        None
    } else {
        Some(text)
    }
}

fn collect_json_files(root: &Path, files: &mut Vec<PathBuf>) {
    if !root.exists() {
        return;
    }

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_json_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            files.push(path);
        }
    }
}

fn collect_json_files_cancellable(
    root: &Path,
    files: &mut Vec<PathBuf>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> bool {
    if is_cancelled() {
        return false;
    }
    if !root.exists() {
        return true;
    }
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return true,
    };
    for entry in entries.flatten() {
        if is_cancelled() {
            return false;
        }
        let path = entry.path();
        if path.is_dir() {
            if !collect_json_files_cancellable(&path, files, is_cancelled) {
                return false;
            }
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            files.push(path);
        }
    }
    true
}

fn find_session_file(storage: &Path, session_id: &str) -> Option<PathBuf> {
    let session_root = storage.join("session");
    let mut files = Vec::new();
    collect_json_files(&session_root, &mut files);
    let expected = format!("{session_id}.json");

    files
        .into_iter()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(expected.as_str()))
}

fn remove_file_if_exists(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn remove_dir_all_if_exists(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;

    fn opencode_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn create_sqlite_schema(conn: &Connection) {
        conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                directory TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL,
                FOREIGN KEY(session_id) REFERENCES session(id) ON DELETE CASCADE
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL,
                FOREIGN KEY(session_id) REFERENCES session(id) ON DELETE CASCADE,
                FOREIGN KEY(message_id) REFERENCES message(id) ON DELETE CASCADE
            );
            ",
        )
        .expect("create sqlite schema");
    }

    #[test]
    fn sqlite_stream_emits_owned_rows_and_observes_cancellation() {
        let conn = Connection::open_in_memory().expect("database");
        create_sqlite_schema(&conn);
        for index in 0..3 {
            conn.execute(
                "INSERT INTO session (id, title, directory, time_created, time_updated)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    format!("ses_{index}"),
                    format!("title-{index}"),
                    "/tmp/project",
                    index,
                    index + 10
                ],
            )
            .expect("insert");
        }
        assert!(sqlite_stream_decodable_contains(&conn, ":memory:", "ses_1").expect("decode"));
        assert!(!sqlite_stream_decodable_contains(&conn, ":memory:", "missing").expect("decode"));

        let mut json_ids = Vec::new();
        let mut duplicates = 0;
        assert!(emit_json_with_sqlite_precedence(
            Some(&conn),
            ":memory:",
            SessionMeta {
                provider_id: PROVIDER_ID.to_string(),
                session_id: "ses_1".to_string(),
                ..SessionMeta::default()
            },
            &mut |meta| {
                json_ids.push(meta.session_id);
                ControlFlow::Continue(())
            },
            &mut duplicates,
        )
        .expect("duplicate lookup")
        .is_continue());
        let _ = emit_json_with_sqlite_precedence(
            Some(&conn),
            ":memory:",
            SessionMeta {
                provider_id: PROVIDER_ID.to_string(),
                session_id: "json-only".to_string(),
                ..SessionMeta::default()
            },
            &mut |meta| {
                json_ids.push(meta.session_id);
                ControlFlow::Continue(())
            },
            &mut duplicates,
        );
        assert_eq!(duplicates, 1);
        assert_eq!(json_ids, vec!["json-only"]);

        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let mut ids = Vec::new();
        let mut stats = cache::StreamScanStats::default();
        let result = stream_sqlite_sessions(
            &conn,
            &mut |meta| {
                ids.push(meta.session_id);
                cancelled.store(true, std::sync::atomic::Ordering::Release);
                ControlFlow::Continue(())
            },
            &|| cancelled.load(std::sync::atomic::Ordering::Acquire),
            &mut stats,
        );
        assert_eq!(result, Err(cache::StreamScanStop::Cancelled));
        assert_eq!(ids.len(), 1);
        assert_eq!(stats.emitted, 1);
    }

    #[test]
    fn sqlite_stream_bounds_text_before_row_decode_and_rejects_bad_schema() {
        let conn = Connection::open_in_memory().expect("database");
        create_sqlite_schema(&conn);
        let oversized = "x".repeat(MAX_METADATA_LINE_BYTES + 1);
        conn.execute(
            "INSERT INTO session (id, title, directory, time_created, time_updated)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                "ses_bounded",
                oversized.as_str(),
                oversized.as_str(),
                1_i64,
                2_i64
            ],
        )
        .expect("insert");
        let mut rows = Vec::new();
        let mut stats = cache::StreamScanStats::default();
        stream_sqlite_sessions(
            &conn,
            &mut |meta| {
                rows.push(meta);
                ControlFlow::Continue(())
            },
            &|| false,
            &mut stats,
        )
        .expect("stream");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].title.is_none());
        assert!(rows[0].project_dir.is_none());

        let malformed = Connection::open_in_memory().expect("database");
        malformed
            .execute_batch("CREATE TABLE session (id TEXT);")
            .expect("schema");
        let mut stats = cache::StreamScanStats::default();
        assert_eq!(
            stream_sqlite_sessions(
                &malformed,
                &mut |_| ControlFlow::Continue(()),
                &|| false,
                &mut stats,
            ),
            Err(cache::StreamScanStop::Incomplete)
        );
    }

    #[test]
    fn delete_session_removes_session_diff_messages_and_parts() {
        let temp = tempdir().expect("tempdir");
        let storage = temp.path();
        let project_id = "project-123";
        let session_id = "ses_123";
        let session_dir = storage.join("session").join(project_id);
        let message_dir = storage.join("message").join(session_id);
        let session_diff = storage
            .join("session_diff")
            .join(format!("{session_id}.json"));
        let part_dir = storage.join("part").join("msg_1");
        let session_file = session_dir.join(format!("{session_id}.json"));

        std::fs::create_dir_all(&session_dir).expect("create session dir");
        std::fs::create_dir_all(&message_dir).expect("create message dir");
        std::fs::create_dir_all(&part_dir).expect("create part dir");
        std::fs::create_dir_all(storage.join("project")).expect("create project dir");
        std::fs::create_dir_all(storage.join("session_diff")).expect("create session diff dir");

        std::fs::write(
            &session_file,
            format!(
                r#"{{
                  "id": "{session_id}",
                  "projectID": "{project_id}",
                  "directory": "/tmp/project",
                  "time": {{ "created": 1, "updated": 2 }}
                }}"#
            ),
        )
        .expect("write session file");
        std::fs::write(
            message_dir.join("msg_1.json"),
            format!(r#"{{"id":"msg_1","sessionID":"{session_id}","role":"user"}}"#),
        )
        .expect("write message file");
        std::fs::write(
            part_dir.join("prt_1.json"),
            r#"{"id":"prt_1","messageID":"msg_1"}"#,
        )
        .expect("write part file");
        std::fs::write(&session_diff, "[]").expect("write session diff");
        std::fs::write(
            storage.join("project").join(format!("{project_id}.json")),
            r#"{"id":"project-123"}"#,
        )
        .expect("write project file");

        delete_session(storage, &message_dir, session_id).expect("delete session");

        assert!(!session_file.exists());
        assert!(!message_dir.exists());
        assert!(!session_diff.exists());
        assert!(!part_dir.exists());
        assert!(storage
            .join("project")
            .join(format!("{project_id}.json"))
            .exists());
    }

    #[test]
    fn load_messages_includes_tool_parts() {
        let temp = tempdir().expect("tempdir");
        let storage = temp.path();
        let session_id = "ses_test";
        let msg_id = "msg_1";

        let msg_dir = storage.join("message").join(session_id);
        let part_dir = storage.join("part").join(msg_id);
        std::fs::create_dir_all(&msg_dir).expect("create msg dir");
        std::fs::create_dir_all(&part_dir).expect("create part dir");

        std::fs::write(
            msg_dir.join(format!("{msg_id}.json")),
            r#"{"id":"msg_1","role":"assistant","time":{"created":"2026-03-06T10:00:00Z"}}"#,
        )
        .expect("write msg");

        std::fs::write(
            part_dir.join("prt_1.json"),
            r#"{"id":"prt_1","type":"tool","tool":"bash","state":{"status":"completed","input":{"command":"ls"},"output":"file.txt"}}"#,
        )
        .expect("write tool part");

        std::fs::write(
            part_dir.join("prt_2.json"),
            r#"{"id":"prt_2","type":"text","text":"Here are the files."}"#,
        )
        .expect("write text part");

        let msgs = load_messages(&msg_dir).expect("load");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "assistant");
        assert!(msgs[0].content.contains("[Tool: bash]"));
        assert!(msgs[0].content.contains("Here are the files."));
    }

    #[test]
    fn parse_sqlite_source_accepts_valid_references() {
        let parsed = parse_sqlite_source("sqlite:/tmp/opencode.db:ses_123").expect("valid source");

        assert_eq!(parsed.0, PathBuf::from("/tmp/opencode.db"));
        assert_eq!(parsed.1, "ses_123");
    }

    #[test]
    fn parse_sqlite_source_rejects_invalid_references() {
        assert!(parse_sqlite_source("/tmp/opencode.db:ses_123").is_none());
        assert!(parse_sqlite_source("sqlite:/tmp/opencode.db:msg_123").is_none());
        assert!(parse_sqlite_source("sqlite:/tmp/opencode.db").is_none());
    }

    #[test]
    #[allow(deprecated)] // set_var/remove_var deprecated since Rust 1.81; safe here under mutex
    fn scan_sessions_sqlite_reads_temp_database() {
        let _guard = opencode_env_lock().lock().expect("lock");
        let temp = tempdir().expect("tempdir");
        let original_xdg = std::env::var_os("XDG_DATA_HOME");
        std::env::set_var("XDG_DATA_HOME", temp.path());

        let base_dir = temp.path().join("opencode");
        std::fs::create_dir_all(&base_dir).expect("create base dir");
        let db_path = base_dir.join("opencode.db");
        let conn = Connection::open(&db_path).expect("open sqlite db");
        create_sqlite_schema(&conn);

        conn.execute(
            "INSERT INTO session (id, title, directory, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, ?5)",
            ("ses_1", "", "/tmp/project-a", 1_771_061_953_033_i64, 1_771_061_954_033_i64),
        )
        .expect("insert session 1");
        conn.execute(
            "INSERT INTO session (id, title, directory, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, ?5)",
            ("ses_2", "Named Session", "/tmp/project-b", 1_771_061_950_000_i64, 1_771_061_955_000_i64),
        )
        .expect("insert session 2");
        drop(conn);

        let sessions = scan_sessions_sqlite();

        #[allow(deprecated)]
        if let Some(value) = original_xdg {
            std::env::set_var("XDG_DATA_HOME", value);
        } else {
            std::env::remove_var("XDG_DATA_HOME");
        }

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, "ses_2");
        assert_eq!(sessions[0].title.as_deref(), Some("Named Session"));
        assert_eq!(sessions[1].session_id, "ses_1");
        assert_eq!(sessions[1].title.as_deref(), Some("project-a"));
        assert_eq!(sessions[1].project_dir.as_deref(), Some("/tmp/project-a"));
        let expected_source = format!("sqlite:{}:ses_1", db_path.display());
        assert_eq!(
            sessions[1].source_path.as_deref(),
            Some(expected_source.as_str())
        );
    }

    #[test]
    fn load_messages_sqlite_reads_messages_and_parts() {
        let temp = tempdir().expect("tempdir");
        let db_path = temp.path().join("opencode.db");
        let conn = Connection::open(&db_path).expect("open sqlite db");
        create_sqlite_schema(&conn);

        conn.execute(
            "INSERT INTO session (id, title, directory, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, ?5)",
            ("ses_1", "Session", "/tmp/project-a", 1000_i64, 3000_i64),
        )
        .expect("insert session");
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
            ("msg_1", "ses_1", 1000_i64, r#"{"role":"user"}"#),
        )
        .expect("insert message 1");
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
            ("msg_2", "ses_1", 2000_i64, r#"{"role":"assistant"}"#),
        )
        .expect("insert message 2");
        conn.execute(
            "INSERT INTO part (id, session_id, message_id, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            ("prt_1", "ses_1", "msg_1", 1000_i64, r#"{"type":"text","text":"Hello"}"#),
        )
        .expect("insert part 1");
        conn.execute(
            "INSERT INTO part (id, session_id, message_id, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                "prt_2",
                "ses_1",
                "msg_2",
                2000_i64,
                r#"{"type":"tool","tool":"bash"}"#,
            ),
        )
        .expect("insert part 2");
        conn.execute(
            "INSERT INTO part (id, session_id, message_id, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                "prt_3",
                "ses_1",
                "msg_2",
                2001_i64,
                r#"{"type":"text","text":"Done"}"#,
            ),
        )
        .expect("insert part 3");
        drop(conn);

        let source = format!("sqlite:{}:ses_1", db_path.display());
        let messages = load_messages_sqlite(&source).expect("load sqlite messages");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "Hello");
        assert_eq!(messages[0].ts, Some(1000));
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content, "[Tool: bash]\nDone");
        assert_eq!(messages[1].ts, Some(2000));
    }

    #[test]
    fn delete_session_sqlite_removes_session() {
        let _guard = opencode_env_lock().lock().expect("lock");
        let temp = tempdir().expect("tempdir");
        let original_xdg = std::env::var_os("XDG_DATA_HOME");
        #[allow(deprecated)]
        std::env::set_var("XDG_DATA_HOME", temp.path());

        let base_dir = temp.path().join("opencode");
        std::fs::create_dir_all(&base_dir).expect("create base dir");
        let db_path = base_dir.join("opencode.db");
        let conn = Connection::open(&db_path).expect("open sqlite db");
        create_sqlite_schema(&conn);

        conn.execute(
            "INSERT INTO session (id, title, directory, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, ?5)",
            ("ses_1", "Session", "/tmp/project-a", 1000_i64, 3000_i64),
        )
        .expect("insert session");
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
            ("msg_1", "ses_1", 1000_i64, r#"{"role":"user"}"#),
        )
        .expect("insert message");
        conn.execute(
            "INSERT INTO part (id, session_id, message_id, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            ("prt_1", "ses_1", "msg_1", 1000_i64, r#"{"type":"text","text":"Hello"}"#),
        )
        .expect("insert part");
        drop(conn);

        let source = format!("sqlite:{}:ses_1", db_path.display());
        let deleted = delete_session_sqlite("ses_1", &source).expect("delete sqlite session");
        assert!(deleted);

        let conn = Connection::open(&db_path).expect("re-open sqlite db");
        let remaining_sessions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session WHERE id = 'ses_1'",
                [],
                |row| row.get(0),
            )
            .expect("count sessions");
        let remaining_messages: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message WHERE session_id = 'ses_1'",
                [],
                |row| row.get(0),
            )
            .expect("count messages");
        let remaining_parts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM part WHERE session_id = 'ses_1'",
                [],
                |row| row.get(0),
            )
            .expect("count parts");

        assert_eq!(remaining_sessions, 0);
        assert_eq!(remaining_messages, 0);
        assert_eq!(remaining_parts, 0);

        #[allow(deprecated)]
        if let Some(value) = original_xdg {
            std::env::set_var("XDG_DATA_HOME", value);
        } else {
            std::env::remove_var("XDG_DATA_HOME");
        }
    }

    #[test]
    fn delete_session_sqlite_rejects_foreign_db_path() {
        let _guard = opencode_env_lock().lock().expect("lock");
        let temp = tempdir().expect("tempdir");
        let original_xdg = std::env::var_os("XDG_DATA_HOME");
        #[allow(deprecated)]
        std::env::set_var("XDG_DATA_HOME", temp.path());

        let expected_base_dir = temp.path().join("opencode");
        std::fs::create_dir_all(&expected_base_dir).expect("create expected base dir");
        let expected_db_path = expected_base_dir.join("opencode.db");
        Connection::open(&expected_db_path).expect("create expected sqlite db");

        let db_path = temp.path().join("foreign.db");
        let conn = Connection::open(&db_path).expect("open sqlite db");
        create_sqlite_schema(&conn);
        conn.execute(
            "INSERT INTO session (id, title, directory, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, ?5)",
            ("ses_1", "Session", "/tmp/project", 1000_i64, 3000_i64),
        )
        .expect("insert session");
        drop(conn);

        let source = format!("sqlite:{}:ses_1", db_path.display());
        let err = delete_session_sqlite("ses_1", &source).expect_err("should reject foreign db");
        assert!(err.contains("expected OpenCode database"));

        #[allow(deprecated)]
        if let Some(value) = original_xdg {
            std::env::set_var("XDG_DATA_HOME", value);
        } else {
            std::env::remove_var("XDG_DATA_HOME");
        }
    }

    /// 契约固化：有显式 title 的会话，parse_session 让 summary == title
    /// （同一字符串 clone），is_cacheable 判定为可缓存。
    #[test]
    fn parse_session_with_explicit_title_has_summary_equal_title_and_is_cacheable() {
        let temp = tempdir().expect("tempdir");
        let storage = temp.path();
        let session_id = "ses_titled";
        let session_dir = storage.join("session").join("proj");
        std::fs::create_dir_all(&session_dir).expect("create session dir");

        let session_file = session_dir.join(format!("{session_id}.json"));
        std::fs::write(
            &session_file,
            format!(
                r#"{{"id":"{session_id}","title":"My Named Session","directory":"/tmp/some-project","time":{{"created":1,"updated":2}}}}"#
            ),
        )
        .expect("write session");

        let meta = parse_session(storage, &session_file).expect("parse session");
        assert_eq!(meta.title.as_deref(), Some("My Named Session"));
        // 不变量：summary 与 title 同源同值。
        assert_eq!(meta.summary, meta.title);
        assert!(is_cacheable(&meta), "显式 title 的会话应可缓存");
    }

    /// 契约固化：无显式 title 但有 directory、且首条 user 消息的 part 文本非空
    /// 且不等于目录 basename 时，title 来自目录名、summary 来自 part 文本，二者
    /// 不同源不相等，is_cacheable 判定为不可缓存（否则 part 晚到/变化会长期显示
    /// 旧 summary）。
    #[test]
    fn parse_session_without_title_derives_summary_from_parts_and_is_uncacheable() {
        let temp = tempdir().expect("tempdir");
        let storage = temp.path();
        let session_id = "ses_untitled";
        let msg_id = "msg_1";

        let session_dir = storage.join("session").join("proj");
        let message_dir = storage.join("message").join(session_id);
        let part_dir = storage.join("part").join(msg_id);
        std::fs::create_dir_all(&session_dir).expect("create session dir");
        std::fs::create_dir_all(&message_dir).expect("create message dir");
        std::fs::create_dir_all(&part_dir).expect("create part dir");

        // 无 title、有 directory（basename 为 "my-project"）。
        let session_file = session_dir.join(format!("{session_id}.json"));
        std::fs::write(
            &session_file,
            format!(
                r#"{{"id":"{session_id}","directory":"/tmp/my-project","time":{{"created":1,"updated":2}}}}"#
            ),
        )
        .expect("write session");

        // 首条 user 消息 + 一条 part 文本（非空且不等于目录 basename）。
        std::fs::write(
            message_dir.join(format!("{msg_id}.json")),
            format!(
                r#"{{"id":"{msg_id}","sessionID":"{session_id}","role":"user","time":{{"created":10}}}}"#
            ),
        )
        .expect("write message");
        std::fs::write(
            part_dir.join("prt_1.json"),
            r#"{"id":"prt_1","messageID":"msg_1","type":"text","text":"help me fix the parser bug"}"#,
        )
        .expect("write part");

        let meta = parse_session(storage, &session_file).expect("parse session");
        // title 来自目录名，summary 来自 part 文本 —— 不同源、必不相等。
        assert_eq!(meta.title.as_deref(), Some("my-project"));
        assert_eq!(meta.summary.as_deref(), Some("help me fix the parser bug"));
        assert_ne!(meta.summary, meta.title);
        assert!(
            !is_cacheable(&meta),
            "summary 派生自 part 文件的会话不可缓存"
        );
    }

    /// 构造上消除巧合窗口：无显式 title、目录 basename 与首条 user part 文本恰好
    /// 相同。parse_session 应把冗余的 summary 丢弃为 None（无损），is_cacheable
    /// 判定为不可缓存——避免"summary 派生自 part 却因巧合等于 title"被误缓存。
    #[test]
    fn parse_session_without_title_drops_summary_equal_to_title_and_is_uncacheable() {
        let temp = tempdir().expect("tempdir");
        let storage = temp.path();
        let session_id = "ses_coincide";
        let msg_id = "msg_1";

        let session_dir = storage.join("session").join("proj");
        let message_dir = storage.join("message").join(session_id);
        let part_dir = storage.join("part").join(msg_id);
        std::fs::create_dir_all(&session_dir).expect("create session dir");
        std::fs::create_dir_all(&message_dir).expect("create message dir");
        std::fs::create_dir_all(&part_dir).expect("create part dir");

        // 无 title，directory basename 为 "my-project"。
        let session_file = session_dir.join(format!("{session_id}.json"));
        std::fs::write(
            &session_file,
            format!(
                r#"{{"id":"{session_id}","directory":"/tmp/my-project","time":{{"created":1,"updated":2}}}}"#
            ),
        )
        .expect("write session");

        // 首条 user 消息的 part 文本恰好等于目录 basename "my-project"（巧合）。
        std::fs::write(
            message_dir.join(format!("{msg_id}.json")),
            format!(
                r#"{{"id":"{msg_id}","sessionID":"{session_id}","role":"user","time":{{"created":10}}}}"#
            ),
        )
        .expect("write message");
        std::fs::write(
            part_dir.join("prt_1.json"),
            r#"{"id":"prt_1","messageID":"msg_1","type":"text","text":"my-project"}"#,
        )
        .expect("write part");

        let meta = parse_session(storage, &session_file).expect("parse session");
        assert_eq!(meta.title.as_deref(), Some("my-project"));
        // 巧合命中：派生 summary 等于 title → 丢弃为 None。
        assert_eq!(meta.summary, None);
        assert!(
            !is_cacheable(&meta),
            "无显式 title 的会话不可缓存（即便 summary 恰好等于 title）"
        );
    }
}
