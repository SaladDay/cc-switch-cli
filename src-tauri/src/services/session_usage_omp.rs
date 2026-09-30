//! OMP coding-agent session usage importer.
//!
//! OMP records normalized token and cost data in its session JSONL files. This
//! importer keeps direct (non-proxy) OMP usage visible in the shared dashboard.

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::proxy::usage::calculator::CostCalculator;
use crate::proxy::usage::parser::TokenUsage;
use crate::services::session_usage::{
    metadata_modified_nanos, open_session_file_no_follow, update_line_sync_state_conn,
    SessionSyncResult,
};
use crate::services::sql_helpers::INPUT_TOKEN_SEMANTICS_FRESH;
use crate::services::usage_stats::find_model_pricing_match;
use rusqlite::OptionalExtension;
use rust_decimal::Decimal;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::str::FromStr;

const APP_TYPE: &str = "omp";
const DATA_SOURCE: &str = "omp_session";
const PROVIDER_PLACEHOLDER: &str = "_omp_session";
const UNKNOWN_MODEL: &str = "unknown";
const MAX_USAGE_LABEL_BYTES: usize = 512;
const MIN_SQLITE_UNIX_MILLIS: i64 = -62_167_219_200_000;
const MAX_SQLITE_UNIX_MILLIS: i64 = 253_402_300_799_999;
const REVISION_TAIL_BYTES: u64 = 4096;
const REVISION_MARKER_SHIFT: u32 = 61;
const REVISION_COMPLETE_SHIFT: u32 = 60;
const REVISION_SIZE_SHIFT: u32 = 32;
const REVISION_MARKER: u64 = 0b101;
const REVISION_SIZE_MASK: u64 = (1 << 28) - 1;
const OMP_REQUEST_DEDUP_SQL: &str = "SELECT EXISTS(
         SELECT 1 FROM session_usage_dedup
         WHERE data_source = ?1 AND request_id = ?2
     )";
const OMP_SEMANTIC_DEDUP_SQL: &str = "SELECT EXISTS(
         SELECT 1 FROM session_usage_dedup
         WHERE data_source = ?1 AND semantic_id = ?2
     )";
const OMP_LEGACY_SEMANTIC_DEDUP_SQL: &str = "SELECT EXISTS(
         SELECT 1 FROM session_usage_dedup
         WHERE data_source = ?1 AND semantic_id = ?2 AND has_entry_id = 0
     )";

#[derive(Debug, Clone, Copy, Default)]
struct OMPCosts {
    input: Decimal,
    output: Decimal,
    cache_read: Decimal,
    cache_write: Decimal,
    total: Decimal,
}

impl OMPCosts {
    fn reported(self) -> Option<(Decimal, Decimal, Decimal, Decimal, Decimal)> {
        let component_total = self.input + self.output + self.cache_read + self.cache_write;
        let total = if self.total > Decimal::ZERO {
            self.total
        } else {
            component_total
        };
        (total > Decimal::ZERO).then_some((
            self.input,
            self.output,
            self.cache_read,
            self.cache_write,
            total,
        ))
    }
}

#[derive(Debug)]
struct OMPUsageRecord {
    request_id: String,
    semantic_id: String,
    has_entry_id: bool,
    provider_id: String,
    model: String,
    request_model: String,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_write_tokens: u32,
    costs: OMPCosts,
    status_code: i64,
    error_message: Option<String>,
    created_at: i64,
    session_id: String,
}

#[derive(Debug)]
struct ParsedOMPFile {
    records: Vec<OMPUsageRecord>,
    last_complete_line: i64,
    incomplete_tail: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OMPFileRevision {
    modified_nanos: i64,
    file_size: u64,
    tail_fingerprint: u32,
    complete: bool,
}

impl OMPFileRevision {
    fn encoded_tail(self) -> i64 {
        (u64::from(self.tail_fingerprint) | (u64::from(self.complete) << 32)) as i64
    }
}

#[derive(Debug, Clone, Copy)]
struct OMPSyncState {
    revision: OMPFileRevision,
    last_line_offset: i64,
    legacy_revision: bool,
}

#[derive(Debug)]
struct OMPRequestIdentity {
    request_id: String,
    semantic_id: String,
    has_entry_id: bool,
}

/// Import usage from every OMP session file discoverable by the session
/// browser's current root and layout rules.
pub fn sync_omp_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let files = crate::session_manager::providers::omp::session_files()
        .map_err(|error| AppError::Config(format!("无法发现 OMP 会话: {error}")))?;
    Ok(sync_omp_files(db, &files))
}

fn sync_omp_files(db: &Database, files: &[PathBuf]) -> SessionSyncResult {
    let mut result = SessionSyncResult {
        files_scanned: files.len().min(u32::MAX as usize) as u32,
        ..Default::default()
    };

    for file_path in files {
        match sync_single_omp_file(db, file_path) {
            Ok(file_result) => result.merge(file_result),
            Err(error) => {
                let message = format!("{}: {error}", file_path.display());
                log::warn!("[OMP-SYNC] 会话文件解析失败: {message}");
                result.errors.push(message);
            }
        }
    }

    if result.imported > 0 {
        log::info!(
            "[OMP-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, 扫描 {} 个文件",
            result.imported,
            result.skipped,
            result.files_scanned
        );
    }
    result
}

fn sync_single_omp_file(db: &Database, file_path: &Path) -> Result<SessionSyncResult, AppError> {
    let metadata = fs::symlink_metadata(file_path)
        .map_err(|error| AppError::Config(format!("无法读取 OMP 会话文件元数据: {error}")))?;
    if !metadata.file_type().is_file()
        || file_path.extension().and_then(|value| value.to_str()) != Some("jsonl")
    {
        return Err(AppError::Config(
            "OMP 会话路径不是普通 JSONL 文件".to_string(),
        ));
    }
    if metadata.len() > crate::session_manager::providers::omp::MAX_SESSION_BYTES {
        return Err(AppError::Config(format!(
            "OMP 会话文件超过 {} 字节安全上限",
            crate::session_manager::providers::omp::MAX_SESSION_BYTES
        )));
    }

    let file_path_string = file_path.to_string_lossy().to_string();
    let modified = metadata_modified_nanos(&metadata);
    let revision = omp_file_revision(file_path, &metadata, modified)?;
    let previous = get_omp_sync_state(db, &file_path_string)?;
    if let Some(state) = previous {
        if state.revision == revision {
            if state.legacy_revision {
                let conn = lock_conn!(db.conn);
                update_omp_sync_state_on_conn(
                    &conn,
                    &file_path_string,
                    revision,
                    state.last_line_offset,
                )?;
            }
            return Ok(SessionSyncResult::default());
        }
    }

    // Always parse from the beginning. OMP normally appends JSONL, but a
    // session can also be compacted or rewritten in place; an old EOF/tail
    // fingerprint cannot prove that the bytes in the middle are unchanged.
    // Full rescans are safe because the durable request/semantic ledgers make
    // already-imported records idempotent, and they prevent silent usage loss
    // after a rewrite followed by an append.
    let parsed = parse_omp_file(file_path, revision.file_size, modified)?;
    let conn = lock_conn!(db.conn);
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| AppError::Database(format!("启动 OMP 用量导入事务失败: {error}")))?;
    let mut result = SessionSyncResult::default();
    for record in &parsed.records {
        if insert_omp_record(&tx, record)? {
            result.imported = result.imported.saturating_add(1);
        } else {
            result.skipped = result.skipped.saturating_add(1);
        }
    }

    update_omp_sync_state_on_conn(&tx, &file_path_string, revision, parsed.last_complete_line)?;
    tx.commit()
        .map_err(|error| AppError::Database(format!("提交 OMP 用量导入事务失败: {error}")))?;
    if parsed.incomplete_tail {
        result.deferred_files = 1;
    }
    Ok(result)
}

fn get_omp_sync_state(db: &Database, file_path: &str) -> Result<Option<OMPSyncState>, AppError> {
    let conn = lock_conn!(db.conn);
    let row = conn
        .query_row(
            "SELECT last_modified, last_line_offset, last_synced_at,
                    last_byte_offset, last_tail_fingerprint
             FROM session_log_sync WHERE file_path = ?1",
            rusqlite::params![file_path],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| AppError::Database(format!("读取 OMP 会话同步状态失败: {error}")))?;
    let Some((
        modified_nanos,
        last_line_offset,
        legacy_encoded_revision,
        stored_file_size,
        stored_tail_fingerprint,
    )) = row
    else {
        return Ok(None);
    };
    let (file_size, tail_fingerprint, complete, legacy_revision) = if let (
        Some(file_size),
        Some(encoded_tail),
    ) =
        (stored_file_size, stored_tail_fingerprint)
    {
        if file_size < 0 || encoded_tail < 0 {
            return Ok(None);
        }
        let encoded_tail = encoded_tail as u64;
        (
            file_size as u64,
            encoded_tail as u32,
            ((encoded_tail >> 32) & 1) == 1,
            false,
        )
    } else {
        // Databases written by the first OMP importer stored the
        // append-proof revision in last_synced_at. Accept those rows once
        // and migrate them to the dedicated cursor columns on the next
        // successful sync.
        let encoded_revision = legacy_encoded_revision as u64;
        if encoded_revision >> REVISION_MARKER_SHIFT != REVISION_MARKER {
            return Ok(None);
        }
        (
            (encoded_revision >> REVISION_SIZE_SHIFT) & REVISION_SIZE_MASK,
            encoded_revision as u32,
            ((encoded_revision >> REVISION_COMPLETE_SHIFT) & 1) == 1,
            true,
        )
    };
    if file_size > crate::session_manager::providers::omp::MAX_SESSION_BYTES
        || last_line_offset < 0
        || last_line_offset > crate::session_manager::providers::omp::MAX_TREE_ENTRIES as i64 + 1
    {
        return Ok(None);
    }
    Ok(Some(OMPSyncState {
        revision: OMPFileRevision {
            modified_nanos,
            file_size,
            tail_fingerprint,
            complete,
        },
        last_line_offset,
        legacy_revision,
    }))
}

fn update_omp_sync_state_on_conn(
    conn: &rusqlite::Connection,
    file_path: &str,
    revision: OMPFileRevision,
    last_line_offset: i64,
) -> Result<(), AppError> {
    update_line_sync_state_conn(
        conn,
        file_path,
        revision.modified_nanos,
        last_line_offset,
        Some(
            i64::try_from(revision.file_size)
                .map_err(|_| AppError::Config("OMP 会话文件大小超出同步游标范围".to_string()))?,
        ),
        Some(revision.encoded_tail()),
    )
}

fn omp_file_revision(
    file_path: &Path,
    metadata: &fs::Metadata,
    modified_nanos: i64,
) -> Result<OMPFileRevision, AppError> {
    let tail_len = metadata.len().min(REVISION_TAIL_BYTES);
    let mut tail = vec![0; tail_len as usize];
    if tail_len > 0 {
        let mut file = open_session_file_no_follow(file_path)
            .map_err(|error| AppError::Config(format!("无法打开 OMP 会话文件: {error}")))?;
        file.seek(SeekFrom::Start(metadata.len() - tail_len))
            .and_then(|_| file.read_exact(&mut tail))
            .map_err(|error| AppError::Config(format!("无法读取 OMP 会话文件尾部: {error}")))?;
    }

    let complete = tail.last() == Some(&b'\n');
    let tail_fingerprint = omp_tail_fingerprint(&tail);
    Ok(OMPFileRevision {
        modified_nanos,
        file_size: metadata.len(),
        tail_fingerprint,
        complete,
    })
}

fn omp_tail_fingerprint(tail: &[u8]) -> u32 {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, b"pi-session-tail-v1");
    hash_field(&mut hasher, tail);
    let digest = hasher.finalize();
    u32::from_be_bytes(digest[..4].try_into().unwrap_or_default())
}

fn parse_omp_file(
    file_path: &Path,
    snapshot_size: u64,
    file_modified_nanos: i64,
) -> Result<ParsedOMPFile, AppError> {
    let file = open_session_file_no_follow(file_path)
        .map_err(|error| AppError::Config(format!("无法打开 OMP 会话文件: {error}")))?;
    let mut reader = BufReader::new(file);
    let mut buffer = String::new();
    let mut line_number = 0i64;
    let mut bytes_read = 0u64;
    let mut session_id = None;
    let mut session_timestamp = None;
    let mut records = Vec::new();
    let mut incomplete_tail = false;

    loop {
        buffer.clear();
        let remaining = snapshot_size.saturating_sub(bytes_read);
        if remaining == 0 {
            break;
        }
        let read = Read::by_ref(&mut reader)
            .take(remaining)
            .read_line(&mut buffer)
            .map_err(|error| AppError::Config(format!("无法读取 OMP 会话文件: {error}")))?;
        if read == 0 {
            return Err(AppError::Config("OMP 会话文件在读取期间被截断".to_string()));
        }
        bytes_read = bytes_read.saturating_add(read as u64);
        if bytes_read > crate::session_manager::providers::omp::MAX_SESSION_BYTES {
            return Err(AppError::Config(
                "OMP 会话文件读取时超过安全上限".to_string(),
            ));
        }
        let has_newline = buffer.ends_with('\n');
        let line = buffer.trim();
        let value = if line.is_empty() {
            None
        } else {
            match serde_json::from_str::<Value>(line) {
                Ok(value) => Some(value),
                Err(_) if !has_newline => {
                    incomplete_tail = true;
                    break;
                }
                Err(_) => None,
            }
        };
        line_number = line_number.saturating_add(1);
        if line_number > crate::session_manager::providers::omp::MAX_TREE_ENTRIES as i64 + 1 {
            return Err(AppError::Config(format!(
                "OMP 会话超过 {} 条 entry 安全上限",
                crate::session_manager::providers::omp::MAX_TREE_ENTRIES
            )));
        }
        let Some(value) = value else {
            if !has_newline {
                incomplete_tail = true;
                break;
            }
            continue;
        };

        if session_id.is_none() {
            // OMP may prepend a title and model/thinking-level metadata before
            // the session header. Those records carry no usage and are safe to
            // ignore while locating the authoritative session id.
            if value.get("type").and_then(Value::as_str) != Some("session") {
                continue;
            }
            session_id = value
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| crate::session_manager::providers::omp::is_valid_tree_id(id))
                .map(str::to_string);
            if session_id.is_none() {
                return Err(AppError::Config("OMP 会话 header 缺少 id".to_string()));
            }
            let header_timestamp_millis = value.get("timestamp").and_then(parse_timestamp_millis);
            session_timestamp = header_timestamp_millis.map(|timestamp| timestamp / 1000);
            continue;
        }
        if let Some(record) = parse_usage_record(
            &value,
            session_id.as_deref().unwrap_or_default(),
            session_timestamp,
            file_modified_nanos / 1_000_000_000,
        ) {
            records.push(record);
        }
    }

    if session_id.is_none() && !incomplete_tail {
        return Err(AppError::Config("OMP 会话没有有效 header".to_string()));
    }
    Ok(ParsedOMPFile {
        records,
        last_complete_line: line_number,
        incomplete_tail,
    })
}

fn parse_usage_record(
    entry: &Value,
    session_id: &str,
    session_timestamp: Option<i64>,
    file_timestamp: i64,
) -> Option<OMPUsageRecord> {
    let entry_type = entry.get("type").and_then(Value::as_str)?;
    let (kind, usage_value, message) = match entry_type {
        "message" => {
            let message = entry.get("message")?;
            match message.get("role").and_then(Value::as_str) {
                Some("assistant") => ("assistant", message.get("usage")?, Some(message)),
                Some("toolResult") => ("tool_result", message.get("usage")?, Some(message)),
                _ => return None,
            }
        }
        // OMP persists usage for non-transcript model-control requests (for
        // example automatic thinking-level changes) as a dedicated entry.
        // These requests must be included in usage totals just like assistant
        // transcript messages.
        "model_usage" => ("model_usage", entry.get("usage")?, None),
        "compaction" => ("compaction", entry.get("usage")?, None),
        "branch_summary" => ("branch_summary", entry.get("usage")?, None),
        _ => return None,
    };

    let event_timestamp_millis = entry
        .get("timestamp")
        .and_then(parse_timestamp_millis)
        .or_else(|| {
            message
                .and_then(|value| value.get("timestamp"))
                .and_then(parse_timestamp_millis)
        });
    let input_tokens = token_count(usage_value, "input");
    let output_tokens = token_count(usage_value, "output");
    let cache_read_tokens = token_count(usage_value, "cacheRead");
    let cache_write_tokens = token_count(usage_value, "cacheWrite");
    let costs = parse_costs(usage_value.get("cost"));
    let stop_reason = message
        .and_then(|value| nonempty_string(value.get("stopReason")))
        .or_else(|| nonempty_string(entry.get("stopReason")));
    let failed = matches!(stop_reason, Some("error" | "aborted"));
    if input_tokens == 0
        && output_tokens == 0
        && cache_read_tokens == 0
        && cache_write_tokens == 0
        && costs.reported().is_none()
        && !failed
    {
        return None;
    }

    let (provider_id, model, request_model) = match kind {
        "assistant" => {
            let message = message?;
            // OMP distinguishes the configured provider/model from the
            // upstream route selected by a proxy or fallback adapter.
            let provider = bounded_label(
                message
                    .get("upstreamProvider")
                    .or_else(|| message.get("provider")),
                PROVIDER_PLACEHOLDER,
            );
            let requested = bounded_label(message.get("model"), UNKNOWN_MODEL);
            let actual = message
                .get("upstreamModel")
                .and_then(|value| nonempty_string(Some(value)))
                .or_else(|| nonempty_string(message.get("responseModel")))
                .map(truncate_usage_label)
                .unwrap_or(&requested)
                .to_string();
            (provider, actual, requested)
        }
        "model_usage" => {
            let provider = bounded_label(
                entry
                    .get("upstreamProvider")
                    .or_else(|| entry.get("provider")),
                PROVIDER_PLACEHOLDER,
            );
            let requested = bounded_label(entry.get("model"), UNKNOWN_MODEL);
            let actual = entry
                .get("upstreamModel")
                .and_then(|value| nonempty_string(Some(value)))
                .or_else(|| nonempty_string(entry.get("responseModel")))
                .map(truncate_usage_label)
                .unwrap_or(&requested)
                .to_string();
            (provider, actual, requested)
        }
        _ => (
            PROVIDER_PLACEHOLDER.to_string(),
            UNKNOWN_MODEL.to_string(),
            UNKNOWN_MODEL.to_string(),
        ),
    };

    let created_at = event_timestamp_millis
        .map(|timestamp| timestamp / 1000)
        .or(session_timestamp)
        .unwrap_or(file_timestamp)
        .clamp(MIN_SQLITE_UNIX_MILLIS / 1000, MAX_SQLITE_UNIX_MILLIS / 1000);

    let (status_code, error_message) = if matches!(kind, "assistant" | "model_usage") {
        match stop_reason {
            Some("error") | Some("aborted") => {
                let fallback = if stop_reason == Some("aborted") {
                    "OMP request aborted"
                } else {
                    "OMP request failed"
                };
                let error = message
                    .and_then(|value| nonempty_string(value.get("errorMessage")))
                    .or_else(|| nonempty_string(entry.get("errorMessage")))
                    .unwrap_or(fallback)
                    .chars()
                    .take(4096)
                    .collect();
                (
                    if stop_reason == Some("aborted") {
                        499
                    } else {
                        500
                    },
                    Some(error),
                )
            }
            _ => (200, None),
        }
    } else {
        (200, None)
    };

    let identity = omp_request_identity(entry, kind, usage_value, message, session_id);

    Some(OMPUsageRecord {
        request_id: identity.request_id,
        semantic_id: identity.semantic_id,
        has_entry_id: identity.has_entry_id,
        provider_id,
        model,
        request_model,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        costs,
        status_code,
        error_message,
        created_at,
        session_id: session_id.to_string(),
    })
}

fn nonempty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn bounded_label(value: Option<&Value>, fallback: &str) -> String {
    truncate_usage_label(nonempty_string(value).unwrap_or(fallback)).to_string()
}

fn truncate_usage_label(value: &str) -> &str {
    if value.len() <= MAX_USAGE_LABEL_BYTES {
        return value;
    }
    let mut end = MAX_USAGE_LABEL_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn token_count(usage: &Value, key: &str) -> u32 {
    usage
        .get(key)
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(u32::MAX as u64) as u32
}

fn parse_costs(value: Option<&Value>) -> OMPCosts {
    let decimal = |key| {
        value
            .and_then(|cost| cost.get(key))
            .and_then(parse_decimal)
            .unwrap_or(Decimal::ZERO)
            .max(Decimal::ZERO)
    };
    OMPCosts {
        input: decimal("input"),
        output: decimal("output"),
        cache_read: decimal("cacheRead"),
        cache_write: decimal("cacheWrite"),
        total: decimal("total"),
    }
}

fn parse_decimal(value: &Value) -> Option<Decimal> {
    let raw = match value {
        Value::Number(number) => number.to_string(),
        Value::String(value) => value.clone(),
        _ => return None,
    };
    Decimal::from_str(&raw)
        .or_else(|_| Decimal::from_scientific(&raw))
        .ok()
}

fn parse_timestamp_millis(value: &Value) -> Option<i64> {
    let timestamp = if let Some(timestamp) = value.as_i64() {
        if !(-100_000_000_000..=100_000_000_000).contains(&timestamp) {
            timestamp
        } else {
            timestamp.saturating_mul(1000)
        }
    } else {
        value
            .as_str()
            .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())?
            .timestamp_millis()
    };
    (MIN_SQLITE_UNIX_MILLIS..=MAX_SQLITE_UNIX_MILLIS)
        .contains(&timestamp)
        .then_some(timestamp)
}

fn omp_request_identity(
    entry: &Value,
    kind: &str,
    usage: &Value,
    message: Option<&Value>,
    session_id: &str,
) -> OMPRequestIdentity {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, b"pi-session-semantic-v1");
    hash_field(&mut hasher, kind.as_bytes());

    for (label, value) in [
        (b"entry_timestamp".as_slice(), entry.get("timestamp")),
        (
            b"message_timestamp".as_slice(),
            message.and_then(|value| value.get("timestamp")),
        ),
    ] {
        if let Some(value) = value {
            hash_field(&mut hasher, label);
            hash_json(&mut hasher, value);
        }
    }
    if let Some(message) = message {
        for key in [
            "provider",
            "upstreamProvider",
            "model",
            "upstreamModel",
            "responseModel",
            "responseId",
            "api",
            "toolCallId",
            "toolName",
            "stopReason",
            "errorMessage",
        ] {
            if let Some(value) = message.get(key) {
                hash_field(&mut hasher, key.as_bytes());
                hash_json(&mut hasher, value);
            }
        }
        if let Some(content) = message.get("content") {
            hash_field(&mut hasher, b"content");
            hash_json(&mut hasher, content);
        }
    } else {
        for key in [
            "provider",
            "upstreamProvider",
            "model",
            "upstreamModel",
            "responseModel",
            "stopReason",
            "errorMessage",
        ] {
            if let Some(value) = entry.get(key) {
                hash_field(&mut hasher, key.as_bytes());
                hash_json(&mut hasher, value);
            }
        }
        if let Some(summary) = entry.get("summary") {
            hash_field(&mut hasher, b"summary");
            hash_json(&mut hasher, summary);
        }
    }
    hash_field(&mut hasher, b"usage");
    hash_json(&mut hasher, usage);
    // Entries without a stable OMP `id` fall back to semantic identity. Keep
    // that identity scoped to its session so identical usage events in two
    // different sessions cannot be mistaken for the same request.
    hash_field(&mut hasher, b"session_id");
    hash_field(&mut hasher, session_id.as_bytes());
    let semantic_id = format!("omp_session_semantic:{:x}", hasher.finalize());
    let entry_id = nonempty_string(entry.get("id"));
    let request_id = if let Some(entry_id) = entry_id {
        let mut request_hasher = Sha256::new();
        hash_field(&mut request_hasher, b"pi-session-request-v3");
        hash_field(&mut request_hasher, kind.as_bytes());
        hash_field(&mut request_hasher, entry_id.as_bytes());
        if let Some(timestamp) = entry.get("timestamp") {
            hash_json(&mut request_hasher, timestamp);
        }
        format!("omp_session:{:x}", request_hasher.finalize())
    } else {
        semantic_id.clone()
    };
    OMPRequestIdentity {
        request_id,
        semantic_id,
        has_entry_id: entry_id.is_some(),
    }
}

fn hash_json(hasher: &mut Sha256, value: &Value) {
    match value {
        Value::Null => hash_field(hasher, b"null"),
        Value::Bool(value) => {
            hash_field(hasher, b"bool");
            hash_field(hasher, if *value { b"true" } else { b"false" });
        }
        Value::Number(value) => {
            hash_field(hasher, b"number");
            hash_field(hasher, value.to_string().as_bytes());
        }
        Value::String(value) => {
            hash_field(hasher, b"string");
            hash_field(hasher, value.as_bytes());
        }
        Value::Array(values) => {
            hash_field(hasher, b"array");
            hash_field(hasher, &(values.len() as u64).to_be_bytes());
            for value in values {
                hash_json(hasher, value);
            }
        }
        Value::Object(values) => {
            hash_field(hasher, b"object");
            hash_field(hasher, &(values.len() as u64).to_be_bytes());
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort_unstable();
            for key in keys {
                hash_field(hasher, key.as_bytes());
                hash_json(hasher, &values[key]);
            }
        }
    }
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn insert_omp_record(
    conn: &rusqlite::Connection,
    record: &OMPUsageRecord,
) -> Result<bool, AppError> {
    let request_seen: bool = conn
        .query_row(
            OMP_REQUEST_DEDUP_SQL,
            rusqlite::params![DATA_SOURCE, record.request_id],
            |row| row.get(0),
        )
        .map_err(|error| AppError::Database(format!("查询 OMP 用量去重账本失败: {error}")))?;
    let already_seen = request_seen
        || conn
            .query_row(
                if record.has_entry_id {
                    OMP_LEGACY_SEMANTIC_DEDUP_SQL
                } else {
                    OMP_SEMANTIC_DEDUP_SQL
                },
                rusqlite::params![DATA_SOURCE, record.semantic_id],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Database(format!("查询 OMP 用量去重账本失败: {error}")))?;
    if already_seen {
        return Ok(false);
    }
    conn.execute(
        "INSERT OR IGNORE INTO session_usage_dedup
         (data_source, request_id, semantic_id, has_entry_id)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            DATA_SOURCE,
            record.request_id,
            record.semantic_id,
            i64::from(record.has_entry_id),
        ],
    )
    .map_err(|error| AppError::Database(format!("写入 OMP 用量去重账本失败: {error}")))?;

    let usage = TokenUsage {
        input_tokens: record.input_tokens,
        output_tokens: record.output_tokens,
        cache_read_tokens: record.cache_read_tokens,
        cache_creation_tokens: record.cache_write_tokens,
        model: Some(record.model.clone()),
        message_id: None,
    };
    let (costs, pricing_model) = if let Some(costs) = record.costs.reported() {
        (Some(costs), record.model.clone())
    } else if let Some((matched_model, pricing)) = find_model_pricing_match(conn, &record.model)
        .ok()
        .flatten()
        .map(|matched| (matched.model_id, matched.pricing))
        .or_else(|| {
            find_model_pricing_match(conn, &record.request_model)
                .ok()
                .flatten()
                .map(|matched| (matched.model_id, matched.pricing))
        })
    {
        let calculated =
            CostCalculator::calculate_for_app(APP_TYPE, &usage, &pricing, Decimal::ONE);
        (
            Some((
                calculated.input_cost,
                calculated.output_cost,
                calculated.cache_read_cost,
                calculated.cache_creation_cost,
                calculated.total_cost,
            )),
            matched_model,
        )
    } else {
        (None, String::new())
    };
    let (input_cost, output_cost, cache_read_cost, cache_write_cost, total_cost) =
        costs.unwrap_or((
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
        ));

    conn.execute(
        "INSERT OR IGNORE INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model, pricing_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_token_semantics,
            input_cost_usd, output_cost_usd, cache_read_cost_usd,
            cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at, data_source
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26
        )",
        rusqlite::params![
            record.request_id,
            record.provider_id,
            APP_TYPE,
            record.model,
            record.request_model,
            pricing_model,
            record.input_tokens,
            record.output_tokens,
            record.cache_read_tokens,
            record.cache_write_tokens,
            INPUT_TOKEN_SEMANTICS_FRESH,
            input_cost.to_string(),
            output_cost.to_string(),
            cache_read_cost.to_string(),
            cache_write_cost.to_string(),
            total_cost.to_string(),
            0i64,
            Option::<i64>::None,
            record.status_code,
            record.error_message,
            record.session_id,
            Some(DATA_SOURCE),
            1i64,
            "1.0",
            record.created_at,
            DATA_SOURCE,
        ],
    )
    .map(|changed| changed > 0)
    .map_err(|error| AppError::Database(format!("插入 OMP 会话用量失败: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use std::io::Write;

    #[test]
    fn parses_omp_responses_usage() {
        let entry: Value = serde_json::json!({
            "type": "message",
            "id": "entry-1",
            "timestamp": "2026-09-04T03:14:22.500Z",
            "message": {
                "role": "assistant",
                "provider": "company-1",
                "model": "gpt-5.6-sol",
                "responseModel": "gpt-5.6-sol",
                "usage": {
                    "input": 3689,
                    "output": 14,
                    "cacheRead": 14848,
                    "cacheWrite": 0,
                    "cost": {"total": 0.0209752}
                },
                "stopReason": "stop"
            }
        });
        let record = parse_usage_record(&entry, "session-1", None, 0).expect("usage");
        assert_eq!(record.provider_id, "company-1");
        assert_eq!(record.model, "gpt-5.6-sol");
        assert_eq!(record.input_tokens, 3689);
        assert_eq!(record.output_tokens, 14);
        assert_eq!(record.cache_read_tokens, 14848);
        assert_eq!(record.status_code, 200);
        assert_eq!(record.costs.total, Decimal::from_str("0.0209752").unwrap());
    }

    #[test]
    fn prefers_omp_upstream_provider_and_model() {
        let entry = serde_json::json!({
            "type": "message",
            "id": "entry-upstream",
            "message": {
                "role": "assistant",
                "provider": "company-1",
                "model": "alias-model",
                "upstreamProvider": "upstream-company",
                "upstreamModel": "gpt-5.6-sol-2026-09-04",
                "usage": {"input": 1, "output": 2, "cost": {"total": 0.1}},
                "stopReason": "stop"
            }
        });
        let record = parse_usage_record(&entry, "session-1", None, 0).expect("usage");
        assert_eq!(record.provider_id, "upstream-company");
        assert_eq!(record.model, "gpt-5.6-sol-2026-09-04");
        assert_eq!(record.request_model, "alias-model");
    }

    #[test]
    fn imports_model_usage_entries() {
        let entry = serde_json::json!({
            "type": "model_usage",
            "id": "model-usage-1",
            "timestamp": "2026-09-04T03:14:23Z",
            "provider": "company-1",
            "model": "gpt-5.6-sol",
            "usage": {
                "input": 7,
                "output": 3,
                "cacheRead": 2,
                "cacheWrite": 1,
                "cost": {"total": "0.0125"}
            },
            "stopReason": "stop"
        });
        let record = parse_usage_record(&entry, "session-1", None, 0).expect("usage");
        assert_eq!(record.provider_id, "company-1");
        assert_eq!(record.model, "gpt-5.6-sol");
        assert_eq!(record.input_tokens, 7);
        assert_eq!(record.output_tokens, 3);
        assert_eq!(record.cache_read_tokens, 2);
        assert_eq!(record.cache_write_tokens, 1);
        assert_eq!(record.status_code, 200);
    }

    #[test]
    fn ignores_non_usage_omp_entries() {
        let entry = serde_json::json!({
            "type": "model_change",
            "model": "company-1/gpt-5.6-sol"
        });
        assert!(parse_usage_record(&entry, "session-1", None, 0).is_none());
    }

    #[test]
    fn imports_real_omp_jsonl_with_metadata_prefix_and_deduplicates() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl");
        let lines = [
            serde_json::json!({
                "type": "title",
                "title": "OMP fixture"
            }),
            serde_json::json!({
                "type": "session",
                "version": 3,
                "id": "session-omp-fixture",
                "timestamp": "2026-09-04T03:14:22.500Z",
                "cwd": "/tmp"
            }),
            serde_json::json!({
                "type": "model_change",
                "id": "model-change-1",
                "model": "company-1/gpt-5.6-sol"
            }),
            serde_json::json!({
                "type": "message",
                "id": "assistant-1",
                "timestamp": "2026-09-04T03:14:23.000Z",
                "message": {
                    "role": "assistant",
                    "api": "openai-responses",
                    "provider": "company-1",
                    "model": "gpt-5.6-sol",
                    "responseModel": "gpt-5.6-sol",
                    "usage": {
                        "input": 3689,
                        "output": 14,
                        "cacheRead": 14848,
                        "cacheWrite": 0,
                        "cost": {
                            "input": 0.014756,
                            "output": 0.00028,
                            "cacheRead": 0.0059392,
                            "cacheWrite": 0,
                            "total": 0.0209752
                        }
                    },
                    "stopReason": "stop"
                }
            }),
        ];
        let content = lines
            .iter()
            .map(|line| serde_json::to_string(line).expect("serialize fixture") + "\n")
            .collect::<String>();
        fs::write(&path, content.as_bytes()).expect("write fixture");

        let db = Database::memory()?;
        let first = sync_omp_files(&db, std::slice::from_ref(&path));
        assert_eq!(first.imported, 1);
        assert!(first.errors.is_empty());
        let second = sync_omp_files(&db, std::slice::from_ref(&path));
        assert_eq!(second.imported, 0);
        assert_eq!(second.skipped, 0);

        let conn = lock_conn!(db.conn);
        let row: (String, String, String, i64, i64, i64, String) = conn.query_row(
            "SELECT provider_id, app_type, data_source, input_tokens,
                    output_tokens, cache_read_tokens, total_cost_usd
             FROM proxy_request_logs WHERE request_id LIKE 'omp_session:%'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )?;
        assert_eq!(row.0, "company-1");
        assert_eq!(row.1, APP_TYPE);
        assert_eq!(row.2, DATA_SOURCE);
        assert_eq!(row.3, 3689);
        assert_eq!(row.4, 14);
        assert_eq!(row.5, 14848);
        assert_eq!(row.6, "0.0209752");
        let sync_state: (i64, Option<i64>, Option<i64>) = conn.query_row(
            "SELECT last_synced_at, last_byte_offset, last_tail_fingerprint
             FROM session_log_sync WHERE file_path = ?1",
            rusqlite::params![path.to_string_lossy().as_ref()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert!(sync_state.0 > 0);
        assert_eq!(sync_state.1, Some(content.len() as i64));
        assert!(sync_state.2.is_some());
        Ok(())
    }

    #[test]
    fn appends_and_completes_partial_omp_tail_without_duplicate_imports() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("active.jsonl");
        let header = serde_json::json!({
            "type": "session", "version": 3, "id": "session-active",
            "timestamp": "2026-09-04T03:14:22Z"
        });
        let first = serde_json::json!({
            "type": "message", "id": "assistant-1", "timestamp": "2026-09-04T03:14:23Z",
            "message": {"role": "assistant", "provider": "company-1", "model": "m1",
                         "usage": {"input": 1, "output": 1}, "stopReason": "stop"}
        });
        let second = serde_json::to_string(&serde_json::json!({
            "type": "message", "id": "assistant-2", "timestamp": "2026-09-04T03:14:24Z",
            "message": {"role": "assistant", "provider": "company-1", "model": "m1",
                         "usage": {"input": 2, "output": 2}, "stopReason": "stop"}
        }))
        .expect("serialize second record");
        let header_line = serde_json::to_string(&header).expect("serialize header") + "\n";
        let first_line = serde_json::to_string(&first).expect("serialize first record") + "\n";
        fs::write(&path, format!("{header_line}{first_line}")).expect("write initial session");
        let db = Database::memory()?;
        let initial = sync_omp_files(&db, std::slice::from_ref(&path));
        assert_eq!(initial.imported, 1);

        let split = second.len() / 2;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open partial session");
        file.write_all(&second.as_bytes()[..split])
            .expect("write partial record");
        file.flush().expect("flush partial record");
        drop(file);
        let partial = sync_omp_files(&db, std::slice::from_ref(&path));
        assert_eq!((partial.imported, partial.deferred_files), (0, 1));

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open completed session");
        file.write_all(&second.as_bytes()[split..])
            .expect("write completed record");
        file.write_all(b"\n").expect("write record newline");
        file.flush().expect("flush completed record");
        drop(file);
        // The file is safely rescanned from the beginning; request/semantic
        // ledgers suppress the already-imported first record.
        let completed = sync_omp_files(&db, std::slice::from_ref(&path));
        assert_eq!(completed.imported, 1);
        let total: i64 = lock_conn!(db.conn).query_row(
            "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = ?1",
            rusqlite::params![DATA_SOURCE],
            |row| row.get(0),
        )?;
        assert_eq!(total, 2);
        Ok(())
    }

    #[test]
    fn imports_error_aborted_and_model_usage_statuses() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("statuses.jsonl");
        let lines = [
            serde_json::json!({"type":"session","id":"session-statuses","version":3}),
            serde_json::json!({"type":"model_usage","id":"failed","provider":"company-1","model":"m","usage":{"input":1,"output":0},"stopReason":"error","errorMessage":"bad gateway"}),
            serde_json::json!({"type":"message","id":"aborted","message":{"role":"assistant","provider":"company-1","model":"m","usage":{"input":0,"output":0},"stopReason":"aborted"}}),
        ];
        let content = lines
            .iter()
            .map(|line| serde_json::to_string(line).expect("serialize status record") + "\n")
            .collect::<String>();
        fs::write(&path, content).expect("write status session");
        let db = Database::memory()?;
        assert_eq!(sync_omp_files(&db, std::slice::from_ref(&path)).imported, 2);
        let statuses: Vec<(i64, String)> = lock_conn!(db.conn)
            .prepare("SELECT status_code, error_message FROM proxy_request_logs WHERE data_source = ?1 ORDER BY status_code")?
            .query_map(rusqlite::params![DATA_SOURCE], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        assert_eq!(
            statuses,
            vec![
                (499, "OMP request aborted".into()),
                (500, "bad gateway".into())
            ]
        );
        Ok(())
    }

    #[test]
    fn leaves_pricing_model_empty_when_cost_is_unavailable() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create isolated session root");
        let path = temp.path().join("unpriced.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"session\",\"version\":3,\"id\":\"unpriced-session\"}\n",
                "{\"type\":\"message\",\"id\":\"unpriced-entry\",\"message\":{\"role\":\"assistant\",\"provider\":\"company-1\",\"model\":\"unpriced-omp-model\",\"usage\":{\"input\":3,\"output\":2},\"stopReason\":\"stop\"}}\n"
            ),
        )
        .expect("write unpriced session");
        let db = Database::memory()?;
        assert_eq!(sync_omp_files(&db, std::slice::from_ref(&path)).imported, 1);
        let pricing_model: String = lock_conn!(db.conn).query_row(
            "SELECT pricing_model FROM proxy_request_logs WHERE data_source = ?1",
            rusqlite::params![DATA_SOURCE],
            |row| row.get(0),
        )?;
        assert!(pricing_model.is_empty());
        Ok(())
    }

    #[test]
    #[serial]
    fn sync_usage_reads_migrated_xdg_data_home_sessions() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("create isolated home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let xdg_data_home = home.path().join("xdg-data");
        let sessions_dir = xdg_data_home.join("omp/sessions");
        fs::create_dir_all(&sessions_dir).expect("create migrated OMP sessions directory");
        std::env::set_var("XDG_DATA_HOME", &xdg_data_home);
        let path = sessions_dir.join("migrated.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"session\",\"version\":3,\"id\":\"migrated-session\"}\n",
                "{\"type\":\"message\",\"id\":\"migrated-entry\",\"message\":{\"role\":\"assistant\",\"provider\":\"company-1\",\"model\":\"gpt-5.6\",\"usage\":{\"input\":3,\"output\":2},\"stopReason\":\"stop\"}}\n"
            ),
        )
        .expect("write migrated session");

        let db = Database::memory()?;
        let result = sync_omp_usage(&db)?;
        assert_eq!(result.imported, 1);
        assert_eq!(result.files_scanned, 1);
        assert!(result.errors.is_empty());

        Ok(())
    }
}
