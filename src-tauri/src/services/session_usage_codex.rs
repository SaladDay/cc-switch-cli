//! Codex 会话日志使用追踪
//!
//! 从 ~/.codex/sessions/ 下的 JSONL 会话文件中提取精确 token 使用数据，
//! 替代原有的 state_5.sqlite 估算方案。
//!
//! ## 数据流
//! ```text
//! ~/.codex/sessions/YYYY/MM/DD/*.jsonl → 增量解析 → delta 计算 → 费用计算 → proxy_request_logs 表
//! ```
//!
//! ## 解析的事件类型
//! - `session_meta` → 提取唯一 thread_id（子代理的 session_id 指向父线程）
//! - `turn_context` → 提取当前 model
//! - `event_msg` (type=token_count) → 提取累计 token 用量，计算 delta

use crate::codex_config::get_codex_config_dir;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::proxy::usage::calculator::CostCalculator;
use crate::proxy::usage::parser::TokenUsage;
use crate::services::session_usage::{
    cached_model_pricing, get_sync_state, metadata_modified_nanos, update_sync_state,
    update_sync_state_conn, PricingCache, SessionSyncResult, SESSION_LOG_COMMIT_BATCH,
};
use crate::services::session_usage_driver::{
    save_resume_hint, scan_jsonl_incremental, unchanged_jsonl_identity_is_suspicious,
};
use crate::services::usage_stats::{
    has_suspected_codex_session_duplicate, should_skip_session_insert, DedupKey,
};
use crate::session_manager::scan_cache_store::{ScanCacheStore, SyncResumeHint};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

const CODEX_THREAD_REQUEST_ID_PREFIX: &str = "codex_session:thread-v1";

/// 累计 token 用量（跟踪 total_token_usage 字段）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CumulativeTokens {
    input: u64,
    cached_input: u64,
    output: u64,
}

/// 单次 API 调用的 token 增量
#[derive(Debug, Clone)]
struct DeltaTokens {
    input: u32,
    cached_input: u32,
    output: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TokenCountersSignature {
    input: Option<u64>,
    cached_input: Option<u64>,
    output: Option<u64>,
    reasoning_output: Option<u64>,
    total: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TokenUsageSignature {
    total: Option<TokenCountersSignature>,
    last: Option<TokenCountersSignature>,
}

#[derive(Debug, Clone)]
enum ParentResolution {
    None,
    Parent(String),
    Deferred(String),
}

#[derive(Debug)]
struct ParsedCodexFile {
    line_offset: i64,
    has_billable_tokens: bool,
}

#[derive(Debug, Clone)]
struct RootMeta {
    timestamp: Option<DateTime<Utc>>,
    parent: ParentResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingReason {
    MissingParent(String),
    Stable(String),
    Retryable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingEntry {
    modified: i64,
    size: u64,
    reason: PendingReason,
}

#[derive(Debug, Default)]
struct CodexReplayCaches {
    parent_signatures: HashMap<(PathBuf, i64), Vec<TokenUsageSignature>>,
    pending: HashMap<PathBuf, PendingEntry>,
}

static CODEX_REPLAY_CACHES: OnceLock<Mutex<CodexReplayCaches>> = OnceLock::new();

fn replay_caches() -> &'static Mutex<CodexReplayCaches> {
    CODEX_REPLAY_CACHES.get_or_init(|| Mutex::new(CodexReplayCaches::default()))
}

pub(crate) fn clear_codex_replay_caches() {
    if let Ok(mut caches) = replay_caches().lock() {
        *caches = CodexReplayCaches::default();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ReplayPhase {
    Matching { parent_offset: usize },
    Live,
}

impl DeltaTokens {
    fn is_zero(&self) -> bool {
        self.input == 0 && self.cached_input == 0 && self.output == 0
    }
}

/// 单文件解析时的运行状态
///
/// 可序列化：字节续传时整个状态机存进 sidecar 提示的 `state` JSON，恢复后
/// 无需从第 1 行重放历史事件来重建 `prev_total`/`event_index`。
#[derive(Debug, Serialize, Deserialize)]
struct FileParseState {
    current_model: String,
    prev_total: Option<CumulativeTokens>,
    event_index: u32,
    replay_phase: ReplayPhase,
}

/// 扫描阶段收集的待写记录：先扫描收集、后批量写库，读文件期间不持有连接锁。
struct PendingCodexEntry {
    request_id: String,
    delta: DeltaTokens,
    model: String,
    session_id: Option<String>,
    /// 在扫描（解析）阶段就定死的入库时间戳（Unix 秒）。缺失/非法 timestamp 的
    /// now() 回退发生在入队处而非写库阶段，避免两阶段延迟污染退化输入的时间。
    created_at: i64,
}

type RolloutIndex = HashMap<String, Vec<PathBuf>>;

#[derive(Debug, Default)]
struct CodexFileSyncResult {
    imported: u32,
    skipped: u32,
    suspected_duplicates: u32,
    deferred: bool,
}

fn is_rollout_filename(file_name: &str) -> bool {
    if !file_name.starts_with("rollout-") || !file_name.ends_with(".jsonl") {
        return false;
    }
    let stem = file_name.trim_end_matches(".jsonl");
    stem.get(stem.len().saturating_sub(36)..)
        .is_some_and(|candidate| uuid::Uuid::parse_str(candidate).is_ok())
}

fn is_codex_cursor_path(file_path: &str, codex_dir: &Path) -> bool {
    let path = Path::new(file_path);
    let file_name = file_path.rsplit(['/', '\\']).next().unwrap_or_default();
    if !is_rollout_filename(file_name) {
        return false;
    }

    if path.starts_with(codex_dir.join("sessions"))
        || path.starts_with(codex_dir.join("archived_sessions"))
    {
        return true;
    }

    file_path
        .replace('\\', "/")
        .split('/')
        .any(|segment| matches!(segment, "sessions" | "archived_sessions"))
}

fn sqlite_table_exists(conn: &rusqlite::Connection, table: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
    .map_err(|error| AppError::Database(format!("查询表 {table} 失败: {error}")))
}

fn sqlite_column_exists(
    conn: &rusqlite::Connection,
    table: &str,
    column: &str,
) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        rusqlite::params![table, column],
        |row| row.get(0),
    )
    .map_err(|error| AppError::Database(format!("查询列 {table}.{column} 失败: {error}")))
}

pub(crate) fn reset_codex_usage_on_conn(
    conn: &rusqlite::Connection,
    codex_dir: &Path,
) -> Result<(), AppError> {
    if sqlite_table_exists(conn, "proxy_request_logs")?
        && sqlite_column_exists(conn, "proxy_request_logs", "data_source")?
    {
        conn.execute(
            "DELETE FROM proxy_request_logs WHERE data_source = 'codex_session'",
            [],
        )
        .map_err(|error| AppError::Database(format!("清理 Codex 会话明细失败: {error}")))?;
    }
    if sqlite_table_exists(conn, "usage_daily_rollups")?
        && sqlite_column_exists(conn, "usage_daily_rollups", "provider_id")?
    {
        conn.execute(
            "DELETE FROM usage_daily_rollups WHERE provider_id = '_codex_session'",
            [],
        )
        .map_err(|error| AppError::Database(format!("清理 Codex 用量汇总失败: {error}")))?;
    }
    if sqlite_table_exists(conn, "session_log_sync")?
        && sqlite_column_exists(conn, "session_log_sync", "file_path")?
    {
        let paths = {
            let mut statement = conn
                .prepare("SELECT file_path FROM session_log_sync")
                .map_err(|error| {
                    AppError::Database(format!("读取会话同步 cursor 失败: {error}"))
                })?;
            let paths = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| AppError::Database(format!("查询会话同步 cursor 失败: {error}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    AppError::Database(format!("解析会话同步 cursor 失败: {error}"))
                })?;
            paths
        };
        for file_path in paths
            .into_iter()
            .filter(|path| is_codex_cursor_path(path, codex_dir))
        {
            conn.execute(
                "DELETE FROM session_log_sync WHERE file_path = ?1",
                [file_path],
            )
            .map_err(|error| AppError::Database(format!("清理 Codex 同步 cursor 失败: {error}")))?;
        }
    }
    Ok(())
}

impl Database {
    /// Refuse a manual full-history rebuild once Codex proxy details have been
    /// folded into rollups. Rollups intentionally do not retain request
    /// identity, so re-importing the same rollout files could no longer dedupe
    /// their usage and would double-count history.
    pub(crate) fn ensure_codex_usage_rebuild_safe(&self) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        if !sqlite_table_exists(&conn, "usage_daily_rollups")? {
            return Ok(());
        }
        let has_proxy_rollups: bool = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM usage_daily_rollups
                    WHERE app_type = 'codex'
                      AND provider_id <> '_codex_session'
                )",
                [],
                |row| row.get(0),
            )
            .map_err(|error| {
                AppError::Database(format!("检查 Codex 历史用量是否可安全重建失败: {error}"))
            })?;
        if has_proxy_rollups {
            return Err(AppError::Message(
                "Cannot safely rebuild Codex usage after proxy history has been rolled up; \
                 keep the existing history and use normal session sync instead."
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn reset_codex_usage(&self) -> Result<(), AppError> {
        let codex_dir = get_codex_config_dir();
        let conn = lock_conn!(self.conn);
        conn.execute("SAVEPOINT reset_codex_usage", [])
            .map_err(|error| AppError::Database(format!("开启 Codex 重建事务失败: {error}")))?;
        let result = reset_codex_usage_on_conn(&conn, &codex_dir);
        match result {
            Ok(()) => {
                conn.execute("RELEASE reset_codex_usage", [])
                    .map_err(|error| {
                        AppError::Database(format!("提交 Codex 重建事务失败: {error}"))
                    })?;
                drop(conn);
                clear_codex_replay_caches();
                Ok(())
            }
            Err(error) => {
                conn.execute("ROLLBACK TO reset_codex_usage", []).ok();
                conn.execute("RELEASE reset_codex_usage", []).ok();
                Err(error)
            }
        }
    }
}

fn non_empty_string(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn thread_id_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let candidate = stem.get(stem.len().checked_sub(36)?..)?;
    uuid::Uuid::parse_str(candidate)
        .ok()
        .map(|value| value.hyphenated().to_string())
}

fn explicit_parent_from_meta(payload: &serde_json::Value) -> ParentResolution {
    let forked_from = non_empty_string(payload.get("forked_from_id"));
    let spawned_from = payload
        .get("source")
        .and_then(|source| source.get("subagent"))
        .and_then(|subagent| subagent.get("thread_spawn"))
        .and_then(|spawn| non_empty_string(spawn.get("parent_thread_id")));

    match (forked_from, spawned_from) {
        (None, None) => ParentResolution::None,
        (Some(parent), None) | (None, Some(parent)) => ParentResolution::Parent(parent),
        (Some(forked), Some(spawned)) if forked == spawned => ParentResolution::Parent(forked),
        (Some(forked), Some(spawned)) => ParentResolution::Deferred(format!(
            "forked_from_id ({forked}) 与 thread_spawn.parent_thread_id ({spawned}) 不一致"
        )),
    }
}

fn parse_timestamp(value: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    value
        .and_then(serde_json::Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn parse_signature_counters(value: Option<&serde_json::Value>) -> Option<TokenCountersSignature> {
    let value = value?.as_object()?;
    Some(TokenCountersSignature {
        input: value
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64),
        cached_input: value
            .get("cached_input_tokens")
            .or_else(|| value.get("cache_read_input_tokens"))
            .and_then(serde_json::Value::as_u64),
        output: value
            .get("output_tokens")
            .and_then(serde_json::Value::as_u64),
        reasoning_output: value
            .get("reasoning_output_tokens")
            .and_then(serde_json::Value::as_u64),
        total: value
            .get("total_tokens")
            .and_then(serde_json::Value::as_u64),
    })
}

fn parse_token_signature(info: &serde_json::Value) -> Option<TokenUsageSignature> {
    let total = parse_signature_counters(info.get("total_token_usage"));
    let last = parse_signature_counters(info.get("last_token_usage"));
    (total.is_some() || last.is_some()).then_some(TokenUsageSignature { total, last })
}

fn get_codex_sync_state(db: &Database, file_path: &Path) -> Result<(i64, i64), AppError> {
    let file_path_str = file_path.to_string_lossy().to_string();
    let state = get_sync_state(db, &file_path_str)?;
    if state != (0, 0)
        || file_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            != Some("archived_sessions")
    {
        return Ok(state);
    }

    let Some(file_name) = file_path.file_name().and_then(|name| name.to_str()) else {
        return Ok(state);
    };
    let slash_suffix = format!("/{file_name}");
    let backslash_suffix = format!("\\{file_name}");
    let conn = lock_conn!(db.conn);
    let inherited = conn.query_row(
        "SELECT last_modified, last_line_offset
         FROM session_log_sync
         WHERE file_path <> ?1
           AND (substr(file_path, -length(?2)) = ?2
                OR substr(file_path, -length(?3)) = ?3)
         ORDER BY last_line_offset DESC, last_modified DESC
         LIMIT 1",
        rusqlite::params![file_path_str, slash_suffix, backslash_suffix],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    );
    drop(conn);

    match inherited {
        Ok(inherited) => {
            update_sync_state(db, &file_path_str, inherited.0, inherited.1)?;
            Ok(inherited)
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(state),
        Err(error) => Err(AppError::Database(format!(
            "查询 Codex 归档文件同步状态失败: {error}"
        ))),
    }
}

/// 归一化 Codex 模型名
///
/// 处理规则（按顺序）：
/// 1. 转小写：`GLM-4.6` → `glm-4.6`
/// 2. 剥离 provider 前缀：`openai/gpt-5.4` → `gpt-5.4`
/// 3. 剥离 ISO 日期后缀：`gpt-5.4-2026-03-05` → `gpt-5.4`
/// 4. 剥离紧凑日期后缀：`gpt-5.4-20260305` → `gpt-5.4`
fn normalize_codex_model(raw: &str) -> String {
    // Step 1: 小写
    let mut name = raw.to_lowercase();

    // Step 2: 剥离 "provider/" 前缀（如 openai/, azure/）
    if let Some(pos) = name.rfind('/') {
        name = name[pos + 1..].to_string();
    }

    // Step 3: 剥离 ISO 日期后缀 -YYYY-MM-DD（正好 11 字符）
    if name.len() > 11 && name.is_char_boundary(name.len() - 11) {
        let suffix = &name[name.len() - 11..];
        if suffix.is_ascii()
            && suffix.as_bytes()[0] == b'-'
            && suffix[1..5].chars().all(|c| c.is_ascii_digit())
            && suffix.as_bytes()[5] == b'-'
            && suffix[6..8].chars().all(|c| c.is_ascii_digit())
            && suffix.as_bytes()[8] == b'-'
            && suffix[9..11].chars().all(|c| c.is_ascii_digit())
        {
            name.truncate(name.len() - 11);
        }
    }

    // Step 4: 剥离紧凑日期后缀 -YYYYMMDD（正好 9 字符）
    if name.len() > 9 {
        let parts: Vec<&str> = name.rsplitn(2, '-').collect();
        if parts.len() == 2 {
            if let Some(suffix) = parts.first() {
                if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_digit()) {
                    name = parts[1].to_string();
                }
            }
        }
    }

    name
}

/// 解析 Codex 事件时间戳为 Unix 秒；缺失/非法时回退当前时刻。
///
/// 两阶段扫描（先收集 pending、后批量写库）下，退化输入（缺 timestamp）的
/// now() 回退必须在**入队**（解析附近）完成，否则会被推迟到写库阶段而使时间戳
/// 后移。故本函数在扫描回调入队处调用，`insert_codex_session_entry` 只消费定死
/// 的 created_at，不再自行回退 now()。
fn resolve_codex_created_at(timestamp: Option<&str>) -> i64 {
    timestamp
        .and_then(|ts| {
            chrono::DateTime::parse_from_rfc3339(ts)
                .ok()
                .map(|dt| dt.timestamp())
        })
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        })
}

/// 计算两次累计值之间的 delta
fn compute_delta(prev: &Option<CumulativeTokens>, current: &CumulativeTokens) -> DeltaTokens {
    match prev {
        None => DeltaTokens {
            input: current.input as u32,
            cached_input: current.cached_input as u32,
            output: current.output as u32,
        },
        Some(p) => DeltaTokens {
            input: current.input.saturating_sub(p.input) as u32,
            cached_input: current.cached_input.saturating_sub(p.cached_input) as u32,
            output: current.output.saturating_sub(p.output) as u32,
        },
    }
}

/// 从 JSON Value 中提取累计 token 用量
fn parse_cumulative_tokens(total_usage: &serde_json::Value) -> Option<CumulativeTokens> {
    if total_usage.is_null() || !total_usage.is_object() {
        return None;
    }
    Some(CumulativeTokens {
        input: total_usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cached_input: total_usage
            .get("cached_input_tokens")
            .or_else(|| total_usage.get("cache_read_input_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output: total_usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    })
}

fn root_meta_from_value(value: &serde_json::Value, root_thread_id: Option<&str>) -> RootMeta {
    let payload = value.get("payload").unwrap_or(&serde_json::Value::Null);
    let mut parent = explicit_parent_from_meta(payload);

    let meta_thread_id = non_empty_string(
        payload
            .get("id")
            .or_else(|| payload.get("thread_id"))
            .or_else(|| payload.get("threadId")),
    );
    if let (Some(filename_id), Some(meta_id)) = (root_thread_id, meta_thread_id) {
        if filename_id != meta_id {
            parent = ParentResolution::Deferred(format!(
                "文件名线程 ID ({filename_id}) 与 root meta ID ({meta_id}) 不一致"
            ));
        }
    }

    if let ParentResolution::Parent(parent_id) = &mut parent {
        match uuid::Uuid::parse_str(parent_id) {
            Ok(value) => *parent_id = value.hyphenated().to_string(),
            Err(_) => {
                parent = ParentResolution::Deferred(format!(
                    "显式 parent_thread_id 不是有效 UUID: {parent_id}"
                ));
            }
        }
    }
    if matches!((root_thread_id, &parent), (Some(root), ParentResolution::Parent(parent_id)) if root == parent_id)
    {
        parent = ParentResolution::Deferred("parent_thread_id 与 root_thread_id 相同".to_string());
    }

    RootMeta {
        timestamp: parse_timestamp(value.get("timestamp")),
        parent,
    }
}

fn read_root_meta(
    file_path: &Path,
    root_thread_id: Option<&str>,
) -> Result<Option<RootMeta>, AppError> {
    let file =
        fs::File::open(file_path).map_err(|e| AppError::Config(format!("无法打开文件: {e}")))?;
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else {
            continue;
        };
        if !line.contains("\"session_meta\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(serde_json::Value::as_str) == Some("session_meta") {
            return Ok(Some(root_meta_from_value(&value, root_thread_id)));
        }
    }
    Ok(None)
}

fn parse_codex_file(
    file_path: &Path,
    _root_thread_id: Option<String>,
) -> Result<ParsedCodexFile, AppError> {
    let file =
        fs::File::open(file_path).map_err(|e| AppError::Config(format!("无法打开文件: {e}")))?;
    let reader = BufReader::new(file);
    let mut prev_total: Option<CumulativeTokens> = None;
    let mut line_offset = 0i64;
    let mut has_billable_tokens = false;

    for line_result in reader.lines() {
        line_offset += 1;
        let line = match line_result {
            Ok(line) => line,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }

        let is_event_msg = line.contains("\"event_msg\"");
        let is_turn_context = line.contains("\"turn_context\"");
        let is_session_meta = line.contains("\"session_meta\"");
        if !is_event_msg && !is_turn_context && !is_session_meta {
            continue;
        }
        if is_event_msg && !line.contains("\"token_count\"") {
            continue;
        }

        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let Some(event_type) = value.get("type").and_then(serde_json::Value::as_str) else {
            continue;
        };

        match event_type {
            "session_meta" | "turn_context" => {}
            "event_msg" => {
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                if payload.get("type").and_then(serde_json::Value::as_str) != Some("token_count") {
                    continue;
                }
                let Some(info) = payload.get("info").filter(|info| !info.is_null()) else {
                    continue;
                };
                if parse_token_signature(info).is_none() {
                    continue;
                }

                let (cumulative, is_total) = if let Some(total) = info.get("total_token_usage") {
                    (parse_cumulative_tokens(total), true)
                } else if let Some(last) = info.get("last_token_usage") {
                    (parse_cumulative_tokens(last), false)
                } else {
                    continue;
                };
                let Some(cumulative) = cumulative else {
                    continue;
                };
                let delta = if is_total {
                    let delta = compute_delta(&prev_total, &cumulative);
                    prev_total = Some(cumulative);
                    delta
                } else {
                    DeltaTokens {
                        input: cumulative.input as u32,
                        cached_input: cumulative.cached_input as u32,
                        output: cumulative.output as u32,
                    }
                };
                let delta = DeltaTokens {
                    cached_input: delta.cached_input.min(delta.input),
                    ..delta
                };
                if !delta.is_zero() {
                    has_billable_tokens = true;
                }
            }
            _ => {}
        }
    }

    Ok(ParsedCodexFile {
        line_offset,
        has_billable_tokens,
    })
}

fn parent_signatures_before(
    parent_path: &Path,
    cutoff: DateTime<Utc>,
) -> Result<Vec<TokenUsageSignature>, String> {
    let cache_key = (parent_path.to_path_buf(), cutoff.timestamp_micros());
    if let Ok(caches) = replay_caches().lock() {
        if let Some(signatures) = caches.parent_signatures.get(&cache_key) {
            return Ok(signatures.clone());
        }
    }

    let file = fs::File::open(parent_path)
        .map_err(|error| format!("无法打开父 rollout {}: {error}", parent_path.display()))?;
    let mut signatures = Vec::new();
    let mut max_timestamp: Option<DateTime<Utc>> = None;

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let timestamp = parse_timestamp(value.get("timestamp"));
        if let Some(timestamp) = timestamp {
            max_timestamp = Some(max_timestamp.map_or(timestamp, |current| current.max(timestamp)));
        }
        if value.get("type").and_then(serde_json::Value::as_str) != Some("event_msg")
            || value
                .get("payload")
                .and_then(|payload| payload.get("type"))
                .and_then(serde_json::Value::as_str)
                != Some("token_count")
        {
            continue;
        }
        let Some(info) = value
            .get("payload")
            .and_then(|payload| payload.get("info"))
            .filter(|info| !info.is_null())
        else {
            continue;
        };
        let Some(signature) = parse_token_signature(info) else {
            continue;
        };
        let Some(timestamp) = timestamp else {
            return Err(format!(
                "父 rollout {} 的 token_count 缺少有效 timestamp",
                parent_path.display()
            ));
        };
        if timestamp <= cutoff {
            signatures.push(signature);
        }
    }

    if max_timestamp.is_none_or(|timestamp| timestamp < cutoff) {
        return Err(format!(
            "父 rollout {} 尚未写到 child fork 时刻",
            parent_path.display()
        ));
    }

    if let Ok(mut caches) = replay_caches().lock() {
        caches
            .parent_signatures
            .insert(cache_key, signatures.clone());
    }
    Ok(signatures)
}

fn resolve_parent_signatures(
    parent_id: &str,
    cutoff: DateTime<Utc>,
    rollout_index: &RolloutIndex,
) -> Result<Vec<TokenUsageSignature>, String> {
    let Some(candidates) = rollout_index.get(parent_id) else {
        return Err(format!("找不到父 rollout: {parent_id}"));
    };

    let mut snapshots = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        snapshots.push(parent_signatures_before(candidate, cutoff)?);
    }
    let Some(first) = snapshots.first() else {
        return Err(format!("找不到父 rollout: {parent_id}"));
    };
    if snapshots.iter().skip(1).any(|snapshot| snapshot != first) {
        return Err(format!(
            "父 rollout UUID {parent_id} 对应多个内容不一致的文件"
        ));
    }
    Ok(first.clone())
}

fn mark_deferred(
    file_path: &Path,
    modified: i64,
    size: u64,
    reason: PendingReason,
) -> CodexFileSyncResult {
    let entry = PendingEntry {
        modified,
        size,
        reason,
    };
    let should_warn = replay_caches()
        .lock()
        .ok()
        .and_then(|mut caches| {
            caches
                .pending
                .insert(file_path.to_path_buf(), entry.clone())
        })
        .as_ref()
        != Some(&entry);
    if should_warn {
        let reason = match &entry.reason {
            PendingReason::MissingParent(parent) => format!("找不到父 rollout {parent}"),
            PendingReason::Stable(reason) | PendingReason::Retryable(reason) => reason.clone(),
        };
        log::warn!("[CODEX-SYNC] deferred {}: {reason}", file_path.display());
    }
    CodexFileSyncResult {
        deferred: true,
        ..CodexFileSyncResult::default()
    }
}

/// 同步 Codex 使用数据（从 JSONL 会话日志）
pub fn sync_codex_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let codex_dir = get_codex_config_dir();

    let files = collect_codex_session_files(&codex_dir);
    let rollout_index = build_rollout_index(&files);

    let mut result = SessionSyncResult {
        imported: 0,
        skipped: 0,
        files_scanned: files.len() as u32,
        suspected_duplicates: 0,
        deferred_files: 0,
        errors: vec![],
    };

    if files.is_empty() {
        return Ok(result);
    }

    // 本次同步周期共享的定价缓存，避免每条消息重复查 model_pricing 表。
    let mut pricing_cache = PricingCache::new();

    // sidecar 字节续传提示：打不开时优雅降级为全文件重放路径。
    let resume_store = ScanCacheStore::open()
        .inspect_err(|e| log::debug!("[CODEX-SYNC] sidecar 打开失败，禁用字节续传: {e}"))
        .ok();

    // fix 2：一次性预载全部续传提示（一次全表查询），使每文件的 skip 前身份校验与
    // decide_resume 都是内存查找，零额外 per-file IO。
    let resume_hints = resume_store
        .as_ref()
        .map(|s| s.load_all_sync_resume().unwrap_or_default())
        .unwrap_or_default();

    crate::services::session_usage::sync_progress::add_total(files.len() as u32);

    for (file_path, file_mtime) in &files {
        match sync_single_codex_file(
            db,
            file_path,
            *file_mtime,
            &rollout_index,
            &mut pricing_cache,
            resume_store.as_ref(),
            &resume_hints,
        ) {
            Ok(file_result) => {
                result.imported = result.imported.saturating_add(file_result.imported);
                result.skipped = result.skipped.saturating_add(file_result.skipped);
                result.suspected_duplicates = result
                    .suspected_duplicates
                    .saturating_add(file_result.suspected_duplicates);
                if file_result.deferred {
                    result.deferred_files = result.deferred_files.saturating_add(1);
                }
            }
            Err(e) => {
                let msg = format!("Codex 会话文件解析失败 {}: {e}", file_path.display());
                log::warn!("[CODEX-SYNC] {msg}");
                result.errors.push(msg);
            }
        }
        crate::services::session_usage::sync_progress::add_done(1);
    }

    if result.imported > 0 || result.deferred_files > 0 {
        log::info!(
            "[CODEX-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, deferred {} 个, 扫描 {} 个文件",
            result.imported,
            result.skipped,
            result.deferred_files,
            result.files_scanned
        );
    }

    Ok(result)
}

/// 收集所有 Codex 会话 JSONL 文件，返回 `(路径, mtime 纳秒)` 并按 mtime 降序排序
/// （最近修改的最先入库）。walk 阶段顺带取 mtime，既用于排序也传给后续处理，
/// 避免二次 stat（读取失败记 0，交由 `sync_single_codex_file` 回退处理）。
fn collect_codex_session_files(codex_dir: &Path) -> Vec<(PathBuf, i64)> {
    let mut files: Vec<(PathBuf, i64)> = Vec::new();

    // 1. 扫描 sessions/YYYY/MM/DD/*.jsonl（日期分区目录）
    let sessions_dir = codex_dir.join("sessions");
    if sessions_dir.is_dir() {
        collect_jsonl_recursive(&sessions_dir, &mut files, 0, 3);
    }

    // 2. 扫描 archived_sessions/*.jsonl（扁平归档目录）
    let archived_dir = codex_dir.join("archived_sessions");
    if archived_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&archived_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    push_codex_file(&mut files, path);
                }
            }
        }
    }

    files.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    files
}

fn build_rollout_index(files: &[(PathBuf, i64)]) -> RolloutIndex {
    let mut index = RolloutIndex::new();
    for (path, _) in files {
        if let Some(thread_id) = thread_id_from_filename(path) {
            index.entry(thread_id).or_default().push(path.clone());
        }
    }
    for paths in index.values_mut() {
        paths.sort();
    }
    index
}

/// 递归扫描目录下的 .jsonl 文件（限制最大深度），顺带记录 mtime。
fn collect_jsonl_recursive(
    dir: &Path,
    files: &mut Vec<(PathBuf, i64)>,
    depth: u32,
    max_depth: u32,
) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && depth < max_depth {
            collect_jsonl_recursive(&path, files, depth + 1, max_depth);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            push_codex_file(files, path);
        }
    }
}

/// 记录一个 Codex jsonl 文件及其 mtime（读取失败记 0）。
fn push_codex_file(files: &mut Vec<(PathBuf, i64)>, path: PathBuf) {
    let mtime = fs::metadata(&path)
        .map(|m| metadata_modified_nanos(&m))
        .unwrap_or(0);
    files.push((path, mtime));
}

/// 同步单个 Codex JSONL 文件，返回 (imported, skipped)
///
/// `_file_mtime` is the directory-walk snapshot used for ordering. The file is
/// statted again here because deferred-file stability needs its size and the
/// fresh mtime closes the walk-to-processing append race.
///
/// `resume` 提供 sidecar 字节续传提示：Codex 的行跳过发生在解析之后（需要重放
/// 历史事件重建累计值状态），因此提示除字节位置外还必须携带可反序列化的
/// `FileParseState`；命中时 seek + 恢复状态机，彻底跳过历史行的重解析。
fn sync_single_codex_file(
    db: &Database,
    file_path: &Path,
    _file_mtime: i64,
    rollout_index: &RolloutIndex,
    pricing_cache: &mut PricingCache,
    resume: Option<&ScanCacheStore>,
    resume_hints: &HashMap<String, SyncResumeHint>,
) -> Result<CodexFileSyncResult, AppError> {
    let file_path_str = file_path.to_string_lossy().to_string();
    let metadata = fs::metadata(file_path)
        .map_err(|e| AppError::Config(format!("无法读取文件元数据: {e}")))?;
    // This fresh stat is also needed for deferred-file stability. Use its
    // mtime instead of the directory-walk snapshot so an append between walk
    // and processing cannot be skipped.
    let file_modified = metadata_modified_nanos(&metadata);
    let file_size = metadata.len();
    let (last_modified, last_offset) = get_codex_sync_state(db, file_path)?;
    let hint = resume_hints.get(&file_path_str).cloned();

    if file_modified <= last_modified
        && !unchanged_jsonl_identity_is_suspicious(
            &metadata,
            hint.as_ref(),
            last_modified,
            last_offset,
        )
    {
        return Ok(CodexFileSyncResult::default());
    }

    if let Ok(mut caches) = replay_caches().lock() {
        if let Some(pending) = caches.pending.get(file_path).cloned() {
            if pending.modified == file_modified && pending.size == file_size {
                match &pending.reason {
                    PendingReason::MissingParent(parent) if !rollout_index.contains_key(parent) => {
                        return Ok(CodexFileSyncResult {
                            deferred: true,
                            ..CodexFileSyncResult::default()
                        });
                    }
                    PendingReason::Stable(_) => {
                        return Ok(CodexFileSyncResult {
                            deferred: true,
                            ..CodexFileSyncResult::default()
                        });
                    }
                    PendingReason::Retryable(_) => {
                        caches.pending.remove(file_path);
                    }
                    _ => {
                        caches.pending.remove(file_path);
                    }
                }
            }
        }
    }

    let root_thread_id = match thread_id_from_filename(file_path) {
        Some(root_thread_id) => root_thread_id,
        None => {
            return defer_billable_file_or_advance(
                db,
                file_path,
                file_modified,
                file_size,
                None,
                PendingReason::Stable("文件名缺少有效的尾部 UUID".to_string()),
            );
        }
    };
    let root_meta = match read_root_meta(file_path, Some(&root_thread_id))? {
        Some(root_meta) => root_meta,
        None => {
            return defer_billable_file_or_advance(
                db,
                file_path,
                file_modified,
                file_size,
                Some(root_thread_id),
                PendingReason::Stable("含计费 token 但尚无 session_meta".to_string()),
            );
        }
    };

    let (parent_signatures, initial_replay_phase) = match root_meta.parent {
        ParentResolution::None => (Vec::new(), ReplayPhase::Live),
        ParentResolution::Deferred(reason) => {
            return defer_billable_file_or_advance(
                db,
                file_path,
                file_modified,
                file_size,
                Some(root_thread_id),
                PendingReason::Stable(reason),
            );
        }
        ParentResolution::Parent(parent_id) => {
            let Some(cutoff) = root_meta.timestamp else {
                return defer_billable_file_or_advance(
                    db,
                    file_path,
                    file_modified,
                    file_size,
                    Some(root_thread_id),
                    PendingReason::Stable(
                        "parented rollout 的 root meta 缺少有效 timestamp".to_string(),
                    ),
                );
            };
            match resolve_parent_signatures(&parent_id, cutoff, rollout_index) {
                Ok(signatures) => (signatures, ReplayPhase::Matching { parent_offset: 0 }),
                Err(reason) => {
                    let pending_reason = if rollout_index.contains_key(&parent_id) {
                        PendingReason::Retryable(reason)
                    } else {
                        PendingReason::MissingParent(parent_id)
                    };
                    return defer_billable_file_or_advance(
                        db,
                        file_path,
                        file_modified,
                        file_size,
                        Some(root_thread_id),
                        pending_reason,
                    );
                }
            }
        }
    };

    if let Ok(mut caches) = replay_caches().lock() {
        caches.pending.remove(file_path);
    }

    // 扫描阶段：文件驱动归通用驱动，解析归下面的回调；先收集待写记录，
    // 写库阶段再统一批量落库（读文件期间不持有连接锁）。
    let mut pending: Vec<PendingCodexEntry> = Vec::new();
    let mut replay_skipped = 0u32;

    let outcome = scan_jsonl_incremental(
        file_path,
        file_modified,
        last_modified,
        last_offset,
        hint,
        resume.is_some(),
        || FileParseState {
            current_model: "unknown".to_string(),
            prev_total: None,
            event_index: 0,
            replay_phase: initial_replay_phase.clone(),
        },
        |state, line, is_new| {
            // 快速过滤：在 JSON 反序列化前跳过无关行
            let is_event_msg = line.contains("\"event_msg\"");
            let is_turn_context = line.contains("\"turn_context\"");
            let is_session_meta = line.contains("\"session_meta\"");

            if !is_event_msg && !is_turn_context && !is_session_meta {
                return;
            }
            if is_event_msg && !line.contains("\"token_count\"") {
                return;
            }

            let value: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => return,
            };

            let event_type = match value.get("type").and_then(|t| t.as_str()) {
                Some(t) => t,
                None => return,
            };

            match event_type {
                "turn_context" => {
                    if let Some(payload) = value.get("payload") {
                        // model 可能在 payload.model 或 payload.info.model
                        if let Some(model) = payload
                            .get("model")
                            .or_else(|| payload.get("info").and_then(|info| info.get("model")))
                            .and_then(|v| v.as_str())
                        {
                            state.current_model = normalize_codex_model(model);
                        }
                    }
                }
                "event_msg" => {
                    let payload = match value.get("payload") {
                        Some(p) => p,
                        None => return,
                    };

                    // 只处理 token_count 类型
                    if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
                        return;
                    }

                    let info = match payload.get("info") {
                        Some(i) if !i.is_null() => i,
                        _ => return, // 跳过 info 为 null 的首个事件
                    };
                    let Some(signature) = parse_token_signature(info) else {
                        return;
                    };

                    let replayed = consume_replay_signature(
                        &mut state.replay_phase,
                        &parent_signatures,
                        &signature,
                    );

                    // 提取模型（token_count 事件也可能携带 model）
                    if let Some(model) = info
                        .get("model")
                        .or_else(|| info.get("model_name"))
                        .or_else(|| payload.get("model"))
                        .and_then(|v| v.as_str())
                    {
                        state.current_model = normalize_codex_model(model);
                    }

                    // 优先用 total_token_usage（累计值），fallback 到 last_token_usage（增量值）
                    let (cumulative, is_total) = if let Some(total) = info.get("total_token_usage")
                    {
                        (parse_cumulative_tokens(total), true)
                    } else if let Some(last) = info.get("last_token_usage") {
                        (parse_cumulative_tokens(last), false)
                    } else {
                        return;
                    };

                    let cumulative = match cumulative {
                        Some(c) => c,
                        None => return,
                    };

                    let delta = if is_total {
                        // 累计值模式：计算与上次的 delta
                        let d = compute_delta(&state.prev_total, &cumulative);
                        state.prev_total = Some(cumulative);
                        d
                    } else {
                        // 增量值模式：直接使用 last_token_usage 的值
                        DeltaTokens {
                            input: cumulative.input as u32,
                            cached_input: cumulative.cached_input as u32,
                            output: cumulative.output as u32,
                        }
                    };

                    // 钳制：cached 不应超过 input（防护异常数据）
                    let delta = DeltaTokens {
                        cached_input: delta.cached_input.min(delta.input),
                        ..delta
                    };

                    if !delta.is_zero() {
                        state.event_index = state.event_index.saturating_add(1);
                    }

                    if replayed {
                        if is_new && !delta.is_zero() {
                            replay_skipped = replay_skipped.saturating_add(1);
                        }
                        return;
                    }

                    if delta.is_zero() {
                        return;
                    }

                    // 历史行（仅无续传提示的回退路径）只重放重建状态，不产出记录
                    if !is_new {
                        return;
                    }

                    // 生成唯一 request_id
                    let request_id = format!(
                        "{CODEX_THREAD_REQUEST_ID_PREFIX}:{root_thread_id}:{}",
                        state.event_index
                    );

                    // 在入队处（解析附近）就定死 created_at：缺失/非法 timestamp
                    // 回退 now()，避免两阶段写库时才取 now() 造成退化输入时间戳后移。
                    let created_at =
                        resolve_codex_created_at(value.get("timestamp").and_then(|v| v.as_str()));

                    pending.push(PendingCodexEntry {
                        request_id,
                        delta,
                        model: state.current_model.clone(),
                        session_id: Some(root_thread_id.clone()),
                        created_at,
                    });
                }
                _ => {}
            }
        },
    )?;

    // 文件未变化（mtime 跳过）
    let Some(outcome) = outcome else {
        return Ok(CodexFileSyncResult::default());
    };

    let mut result = CodexFileSyncResult {
        skipped: replay_skipped,
        ..CodexFileSyncResult::default()
    };
    commit_codex_entries_and_cursor(
        db,
        pricing_cache,
        &pending,
        &file_path_str,
        outcome.file_modified,
        outcome.line_offset,
        &mut result,
    )?;

    // 主库进度提交成功后，把字节位置与状态机写回 sidecar（尽力而为）
    save_resume_hint(resume, &file_path_str, &outcome);

    Ok(result)
}

fn consume_replay_signature(
    phase: &mut ReplayPhase,
    parent: &[TokenUsageSignature],
    signature: &TokenUsageSignature,
) -> bool {
    let ReplayPhase::Matching { parent_offset } = phase else {
        return false;
    };
    if let Some(relative_match) = parent[*parent_offset..]
        .iter()
        .position(|candidate| candidate == signature)
    {
        *parent_offset += relative_match + 1;
        true
    } else {
        *phase = ReplayPhase::Live;
        false
    }
}

fn defer_billable_file_or_advance(
    db: &Database,
    file_path: &Path,
    file_modified: i64,
    file_size: u64,
    root_thread_id: Option<String>,
    reason: PendingReason,
) -> Result<CodexFileSyncResult, AppError> {
    let parsed = parse_codex_file(file_path, root_thread_id)?;
    if parsed.has_billable_tokens {
        return Ok(mark_deferred(file_path, file_modified, file_size, reason));
    }
    update_sync_state(
        db,
        &file_path.to_string_lossy(),
        file_modified,
        parsed.line_offset,
    )?;
    Ok(CodexFileSyncResult::default())
}

fn commit_codex_entries_and_cursor(
    db: &Database,
    pricing_cache: &mut PricingCache,
    pending: &[PendingCodexEntry],
    file_path: &str,
    file_modified: i64,
    line_offset: i64,
    result: &mut CodexFileSyncResult,
) -> Result<(), AppError> {
    let mut guard = lock_conn!(db.conn);
    let mut tx = guard
        .transaction()
        .map_err(|e| AppError::Database(format!("开启事务失败: {e}")))?;
    let mut since_commit: u32 = 0;

    for entry in pending {
        match insert_codex_session_entry(
            &tx,
            pricing_cache,
            &entry.request_id,
            &entry.delta,
            &entry.model,
            entry.session_id.as_deref(),
            entry.created_at,
            &mut result.suspected_duplicates,
        ) {
            Ok(true) => result.imported = result.imported.saturating_add(1),
            Ok(false) => result.skipped = result.skipped.saturating_add(1),
            Err(e) => {
                log::warn!("[CODEX-SYNC] 插入失败 ({}): {e}", entry.request_id);
                result.skipped = result.skipped.saturating_add(1);
            }
        }

        since_commit = since_commit.saturating_add(1);
        if since_commit >= SESSION_LOG_COMMIT_BATCH {
            tx.commit()
                .map_err(|e| AppError::Database(format!("提交事务失败: {e}")))?;
            tx = guard
                .transaction()
                .map_err(|e| AppError::Database(format!("开启事务失败: {e}")))?;
            since_commit = 0;
        }
    }

    update_sync_state_conn(&tx, file_path, file_modified, line_offset)?;
    tx.commit()
        .map_err(|e| AppError::Database(format!("提交事务失败: {e}")))?;
    Ok(())
}

/// 插入单条 Codex 会话记录到 proxy_request_logs
///
/// 调用方在同一事务连接上批量调用本函数；INSERT 与去重查询走 prepare_cached，
/// 费用查询走 per-cycle 定价缓存。
fn insert_codex_session_entry(
    conn: &rusqlite::Connection,
    pricing_cache: &mut PricingCache,
    request_id: &str,
    delta: &DeltaTokens,
    model: &str,
    session_id: Option<&str>,
    created_at: i64,
    suspected_duplicates: &mut u32,
) -> Result<bool, AppError> {
    // created_at 由调用方在扫描入队处解析定死（见 resolve_codex_created_at），
    // 这里只消费固定值，不再回退 now()。
    if let Some(session_id) = session_id {
        if crate::services::session_identity::attach_imported_session_identity(
            conn,
            &crate::app_config::AppType::Codex,
            request_id,
            session_id,
        )? {
            return Ok(false);
        }
    }
    let dedup_key = DedupKey {
        app_type: "codex",
        model,
        input_tokens: delta.input,
        output_tokens: delta.output,
        cache_read_tokens: delta.cached_input,
        cache_creation_tokens: 0,
        created_at,
    };
    if should_skip_session_insert(conn, request_id, &dedup_key)? {
        return Ok(false);
    }
    if has_suspected_codex_session_duplicate(conn, request_id, &dedup_key)? {
        *suspected_duplicates = suspected_duplicates.saturating_add(1);
        log::warn!(
            "[CODEX-SYNC] 疑似重复会话用量: request_id={request_id}, model={model}, input={}, output={}, cache_read={}",
            delta.input,
            delta.output,
            delta.cached_input
        );
    }

    // 计算费用
    let usage = TokenUsage {
        input_tokens: delta.input,
        output_tokens: delta.output,
        cache_read_tokens: delta.cached_input,
        cache_creation_tokens: 0,
        model: Some(model.to_string()),
        message_id: None,
    };

    // model 在调用处已 normalize_codex_model，缓存键直接使用归一化后的名字。
    let pricing = cached_model_pricing(conn, pricing_cache, model);
    let multiplier = Decimal::from(1);
    let (input_cost, output_cost, cache_read_cost, cache_creation_cost, total_cost) = match pricing
    {
        Some(p) => {
            let cost = CostCalculator::calculate_for_app("codex", &usage, &p, multiplier);
            (
                cost.input_cost.to_string(),
                cost.output_cost.to_string(),
                cost.cache_read_cost.to_string(),
                cost.cache_creation_cost.to_string(),
                cost.total_cost.to_string(),
            )
        }
        None => (
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
        ),
    };

    let mut stmt = conn
        .prepare_cached(
            "INSERT OR IGNORE INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at, data_source
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
        )
        .map_err(|e| AppError::Database(format!("插入 Codex 会话日志失败: {e}")))?;
    let inserted_rows = stmt
        .execute(rusqlite::params![
            request_id,
            "_codex_session", // provider_id
            "codex",          // app_type
            model,
            model, // request_model = model
            delta.input,
            delta.output,
            delta.cached_input,
            0i64, // cache_creation_tokens: Codex 日志无此数据
            input_cost,
            output_cost,
            cache_read_cost,
            cache_creation_cost,
            total_cost,
            0i64,                   // latency_ms
            Option::<i64>::None,    // first_token_ms
            200i64,                 // status_code
            Option::<String>::None, // error_message
            session_id.map(|s| s.to_string()),
            Some("codex_session"), // provider_type
            1i64,                  // is_streaming
            "1.0",                 // cost_multiplier
            created_at,
            "codex_session", // data_source
        ])
        .map_err(|e| AppError::Database(format!("插入 Codex 会话日志失败: {e}")))?;

    // INSERT OR IGNORE 被并发进程抢先时未写入行，计为 skipped 而非 imported
    Ok(inserted_rows > 0)
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const PARENT_ID: &str = "00000000-0000-4000-8000-000000000001";
    const CHILD_A_ID: &str = "00000000-0000-4000-8000-000000000002";
    const CHILD_B_ID: &str = "00000000-0000-4000-8000-000000000003";

    fn write_jsonl(path: &Path, values: &[serde_json::Value]) {
        let contents = values
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(path, contents).unwrap();
    }

    fn rollout_path(dir: &Path, thread_id: &str) -> PathBuf {
        dir.join(format!("rollout-2026-07-10T03-00-00-{thread_id}.jsonl"))
    }

    fn session_meta_at(
        thread_id: &str,
        forked_from_id: Option<&str>,
        spawned_from_id: Option<&str>,
        timestamp: &str,
    ) -> serde_json::Value {
        let source = spawned_from_id.map_or_else(
            || serde_json::Value::String("cli".to_string()),
            |parent| {
                serde_json::json!({
                    "subagent": {
                        "thread_spawn": { "parent_thread_id": parent }
                    }
                })
            },
        );
        serde_json::json!({
            "timestamp": timestamp,
            "type": "session_meta",
            "payload": {
                "id": thread_id,
                "forked_from_id": forked_from_id,
                "source": source
            }
        })
    }

    fn session_meta(thread_id: &str) -> serde_json::Value {
        session_meta_at(thread_id, None, None, "2026-07-10T03:00:00Z")
    }

    fn turn_context_at(timestamp: &str) -> serde_json::Value {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "turn_context",
            "payload": { "model": "gpt-5.6-sol" }
        })
    }

    fn turn_context() -> serde_json::Value {
        turn_context_at("2026-07-10T03:00:01Z")
    }

    fn token_count_at(input: u64, cached: u64, output: u64, timestamp: &str) -> serde_json::Value {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": { "total_token_usage": {
                    "input_tokens": input,
                    "cached_input_tokens": cached,
                    "output_tokens": output,
                    "reasoning_output_tokens": 0,
                    "total_tokens": input + output
                }}
            }
        })
    }

    fn token_count(input: u64, cached: u64, output: u64) -> serde_json::Value {
        token_count_at(input, cached, output, "2026-07-10T03:00:02Z")
    }

    fn sync_test_file(
        db: &Database,
        file: &Path,
        all_files: &[&Path],
    ) -> Result<CodexFileSyncResult, AppError> {
        let files = all_files
            .iter()
            .map(|path| {
                let path = path.to_path_buf();
                let modified = fs::metadata(&path)
                    .map(|metadata| metadata_modified_nanos(&metadata))
                    .unwrap_or(0);
                (path, modified)
            })
            .collect::<Vec<_>>();
        let file_modified = files
            .iter()
            .find_map(|(path, modified)| (path == file).then_some(*modified))
            .unwrap_or(0);
        let rollout_index = build_rollout_index(&files);
        let mut pricing_cache = PricingCache::new();
        sync_single_codex_file(
            db,
            file,
            file_modified,
            &rollout_index,
            &mut pricing_cache,
            None,
            &HashMap::new(),
        )
    }

    fn insert_test_codex_session_entry(
        db: &Database,
        request_id: &str,
        delta: &DeltaTokens,
        model: &str,
        session_id: Option<&str>,
        timestamp: Option<&str>,
        suspected_duplicates: &mut u32,
    ) -> Result<bool, AppError> {
        let conn = lock_conn!(db.conn);
        let mut pricing_cache = PricingCache::new();
        insert_codex_session_entry(
            &conn,
            &mut pricing_cache,
            request_id,
            delta,
            model,
            session_id,
            resolve_codex_created_at(timestamp),
            suspected_duplicates,
        )
    }

    #[test]
    fn test_delta_first_event() {
        let prev = None;
        let current = CumulativeTokens {
            input: 17934,
            cached_input: 9600,
            output: 454,
        };
        let delta = compute_delta(&prev, &current);
        assert_eq!(delta.input, 17934);
        assert_eq!(delta.cached_input, 9600);
        assert_eq!(delta.output, 454);
        assert!(!delta.is_zero());
    }

    #[test]
    fn test_delta_subsequent_event() {
        let prev = Some(CumulativeTokens {
            input: 17934,
            cached_input: 9600,
            output: 454,
        });
        let current = CumulativeTokens {
            input: 36722,
            cached_input: 27904,
            output: 804,
        };
        let delta = compute_delta(&prev, &current);
        assert_eq!(delta.input, 36722 - 17934);
        assert_eq!(delta.cached_input, 27904 - 9600);
        assert_eq!(delta.output, 804 - 454);
    }

    #[test]
    fn test_delta_zero_at_task_boundary() {
        let prev = Some(CumulativeTokens {
            input: 58346,
            cached_input: 46976,
            output: 1045,
        });
        // task 边界：相同的累计值
        let current = CumulativeTokens {
            input: 58346,
            cached_input: 46976,
            output: 1045,
        };
        let delta = compute_delta(&prev, &current);
        assert!(delta.is_zero());
    }

    #[test]
    fn test_delta_saturating_sub() {
        // 异常情况：当前值小于前值（不应发生，但需防护）
        let prev = Some(CumulativeTokens {
            input: 100,
            cached_input: 50,
            output: 30,
        });
        let current = CumulativeTokens {
            input: 80,
            cached_input: 40,
            output: 20,
        };
        let delta = compute_delta(&prev, &current);
        assert_eq!(delta.input, 0);
        assert_eq!(delta.cached_input, 0);
        assert_eq!(delta.output, 0);
        assert!(delta.is_zero());
    }

    #[test]
    fn test_parse_cumulative_tokens_valid() {
        let json: serde_json::Value = serde_json::json!({
            "input_tokens": 17934,
            "cached_input_tokens": 9600,
            "output_tokens": 454,
            "reasoning_output_tokens": 233,
            "total_tokens": 18388
        });
        let tokens = parse_cumulative_tokens(&json).unwrap();
        assert_eq!(tokens.input, 17934);
        assert_eq!(tokens.cached_input, 9600);
        assert_eq!(tokens.output, 454);
    }

    #[test]
    fn test_parse_cumulative_tokens_null() {
        let json = serde_json::Value::Null;
        assert!(parse_cumulative_tokens(&json).is_none());
    }

    #[test]
    fn test_parse_cumulative_tokens_alt_field_names() {
        // 某些版本可能使用 cache_read_input_tokens 而非 cached_input_tokens
        let json: serde_json::Value = serde_json::json!({
            "input_tokens": 1000,
            "cache_read_input_tokens": 500,
            "output_tokens": 200
        });
        let tokens = parse_cumulative_tokens(&json).unwrap();
        assert_eq!(tokens.cached_input, 500);
    }

    #[test]
    fn test_collect_codex_session_files_nonexistent() {
        let files = collect_codex_session_files(Path::new("/nonexistent/path"));
        assert!(files.is_empty());
    }

    #[test]
    fn test_thread_spawn_parent_strips_replay_and_keeps_live_usage() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:01Z"),
                turn_context_at("2026-07-10T03:00:10Z"),
            ],
        );
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, None, Some(PARENT_ID), "2026-07-10T03:00:05Z"),
                turn_context(),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:06Z"),
                token_count_at(1_300, 1_050, 150, "2026-07-10T03:00:07Z"),
            ],
        );

        let result = sync_test_file(&db, &child, &[&parent, &child])?;
        assert_eq!(
            (result.imported, result.skipped, result.deferred),
            (1, 1, false)
        );

        let conn = lock_conn!(db.conn);
        let usage: (i64, i64, i64) = conn.query_row(
            "SELECT input_tokens, cache_read_tokens, output_tokens
             FROM proxy_request_logs WHERE request_id = ?1",
            [format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_A_ID}:2")],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(usage, (300, 150, 50));
        Ok(())
    }

    #[test]
    fn test_incremental_resume_keeps_replay_prefix_alignment() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let resume_store = ScanCacheStore::in_memory()?;
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(100, 50, 10, "2026-07-10T03:00:01Z"),
                token_count_at(200, 100, 20, "2026-07-10T03:00:02Z"),
                turn_context_at("2026-07-10T03:00:10Z"),
            ],
        );
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, Some(PARENT_ID), None, "2026-07-10T03:00:05Z"),
                turn_context(),
                token_count_at(100, 50, 10, "2026-07-10T03:00:06Z"),
            ],
        );

        let files = vec![
            (
                parent.clone(),
                metadata_modified_nanos(&fs::metadata(&parent).unwrap()),
            ),
            (
                child.clone(),
                metadata_modified_nanos(&fs::metadata(&child).unwrap()),
            ),
        ];
        let rollout_index = build_rollout_index(&files);
        let mut pricing_cache = PricingCache::new();
        let first = sync_single_codex_file(
            &db,
            &child,
            files[1].1,
            &rollout_index,
            &mut pricing_cache,
            Some(&resume_store),
            &HashMap::new(),
        )?;
        assert_eq!((first.imported, first.skipped), (0, 1));

        let child_key = child.to_string_lossy().to_string();
        let first_hint = resume_store
            .load_sync_resume(&child_key)?
            .expect("resume hint after first pass");
        let first_state: FileParseState =
            serde_json::from_str(first_hint.state.as_deref().expect("Codex parser state"))
                .expect("deserialize Codex parser state");
        assert!(matches!(
            first_state.replay_phase,
            ReplayPhase::Matching { parent_offset: 1 }
        ));

        std::thread::sleep(std::time::Duration::from_millis(2));
        let mut file = fs::OpenOptions::new().append(true).open(&child).unwrap();
        use std::io::Write as _;
        writeln!(
            file,
            "{}",
            token_count_at(200, 100, 20, "2026-07-10T03:00:07Z")
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            token_count_at(300, 150, 30, "2026-07-10T03:00:08Z")
        )
        .unwrap();
        drop(file);

        let child_mtime = metadata_modified_nanos(&fs::metadata(&child).unwrap());
        let hints = resume_store.load_all_sync_resume()?;
        let second = sync_single_codex_file(
            &db,
            &child,
            child_mtime,
            &rollout_index,
            &mut pricing_cache,
            Some(&resume_store),
            &hints,
        )?;
        assert_eq!((second.imported, second.skipped), (1, 1));

        let conn = lock_conn!(db.conn);
        let usage: (i64, i64, i64) = conn.query_row(
            "SELECT input_tokens, cache_read_tokens, output_tokens
             FROM proxy_request_logs WHERE request_id = ?1",
            [format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_A_ID}:3")],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(usage, (100, 50, 10));
        Ok(())
    }

    #[test]
    fn test_filtered_parent_events_use_subsequence_prefix_alignment() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(100, 50, 10, "2026-07-10T03:00:01Z"),
                token_count_at(200, 100, 20, "2026-07-10T03:00:02Z"),
                token_count_at(300, 150, 30, "2026-07-10T03:00:03Z"),
                turn_context_at("2026-07-10T03:00:10Z"),
            ],
        );
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, Some(PARENT_ID), None, "2026-07-10T03:00:05Z"),
                token_count_at(100, 50, 10, "2026-07-10T03:00:06Z"),
                token_count_at(300, 150, 30, "2026-07-10T03:00:07Z"),
                token_count_at(450, 220, 45, "2026-07-10T03:00:08Z"),
            ],
        );

        let result = sync_test_file(&db, &child, &[&parent, &child])?;
        assert_eq!((result.imported, result.skipped), (1, 2));
        Ok(())
    }

    #[test]
    fn test_empty_fork_imports_no_parent_usage() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(100, 50, 10, "2026-07-10T03:00:01Z"),
                token_count_at(200, 100, 20, "2026-07-10T03:00:02Z"),
                turn_context_at("2026-07-10T03:00:10Z"),
            ],
        );
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, Some(PARENT_ID), None, "2026-07-10T03:00:05Z"),
                token_count_at(100, 50, 10, "2026-07-10T03:00:06Z"),
                token_count_at(200, 100, 20, "2026-07-10T03:00:07Z"),
                serde_json::json!({
                    "timestamp": "2026-07-10T03:00:08Z",
                    "type": "event_msg",
                    "payload": { "type": "thread_settings_applied" }
                }),
            ],
        );

        let result = sync_test_file(&db, &child, &[&parent, &child])?;
        assert_eq!(
            (result.imported, result.skipped, result.deferred),
            (0, 2, false)
        );
        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = 'codex_session'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(count, 0);
        Ok(())
    }

    #[test]
    fn test_conflicting_explicit_parents_are_deferred() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &child,
            &[
                session_meta_at(
                    CHILD_A_ID,
                    Some(PARENT_ID),
                    Some(CHILD_B_ID),
                    "2026-07-10T03:00:05Z",
                ),
                token_count_at(100, 50, 10, "2026-07-10T03:00:06Z"),
            ],
        );

        let result = sync_test_file(&db, &child, &[&child])?;
        assert!(result.deferred);
        assert_eq!(get_sync_state(&db, &child.to_string_lossy())?, (0, 0));
        Ok(())
    }

    #[test]
    fn test_parent_future_signature_cannot_extend_replay_prefix() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(100, 50, 10, "2026-07-10T03:00:01Z"),
                token_count_at(200, 100, 20, "2026-07-10T03:00:06Z"),
            ],
        );
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, Some(PARENT_ID), None, "2026-07-10T03:00:05Z"),
                token_count_at(200, 100, 20, "2026-07-10T03:00:07Z"),
            ],
        );

        let result = sync_test_file(&db, &child, &[&parent, &child])?;
        assert_eq!(
            (result.imported, result.skipped, result.deferred),
            (1, 0, false)
        );
        Ok(())
    }

    #[test]
    fn test_missing_parent_is_deferred_and_recovered_without_child_change() -> Result<(), AppError>
    {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, None, Some(PARENT_ID), "2026-07-10T03:00:05Z"),
                token_count_at(900, 400, 90, "2026-07-10T03:00:06Z"),
            ],
        );

        let deferred = sync_test_file(&db, &child, &[&child])?;
        assert!(deferred.deferred);
        assert_eq!(get_sync_state(&db, &child.to_string_lossy())?, (0, 0));

        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(100, 50, 10, "2026-07-10T03:00:01Z"),
                turn_context_at("2026-07-10T03:00:10Z"),
            ],
        );
        let recovered = sync_test_file(&db, &child, &[&parent, &child])?;
        assert_eq!((recovered.imported, recovered.deferred), (1, false));
        Ok(())
    }

    #[test]
    fn test_billable_file_without_meta_is_deferred_without_cursor() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(&child, &[turn_context(), token_count(100, 50, 10)]);

        let result = sync_test_file(&db, &child, &[&child])?;
        assert!(result.deferred);
        assert_eq!(get_sync_state(&db, &child.to_string_lossy())?, (0, 0));

        std::thread::sleep(std::time::Duration::from_millis(2));
        write_jsonl(
            &child,
            &[
                turn_context(),
                token_count(100, 50, 10),
                session_meta_at(CHILD_A_ID, None, None, "2026-07-10T03:00:03Z"),
            ],
        );
        let recovered = sync_test_file(&db, &child, &[&child])?;
        assert_eq!((recovered.imported, recovered.deferred), (1, false));
        Ok(())
    }

    #[test]
    fn test_non_billable_file_without_meta_advances_cursor() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &child,
            &[
                turn_context(),
                token_count_at(0, 0, 0, "2026-07-10T03:00:02Z"),
            ],
        );

        let result = sync_test_file(&db, &child, &[&child])?;
        assert!(!result.deferred);
        assert_eq!(get_sync_state(&db, &child.to_string_lossy())?.1, 2);
        Ok(())
    }

    #[test]
    fn test_subagents_use_filename_thread_ids() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let child_a = rollout_path(temp.path(), CHILD_A_ID);
        let child_b = rollout_path(temp.path(), CHILD_B_ID);
        write_jsonl(
            &child_a,
            &[
                session_meta(CHILD_A_ID),
                turn_context(),
                token_count(100, 50, 10),
            ],
        );
        write_jsonl(
            &child_b,
            &[
                session_meta(CHILD_B_ID),
                turn_context(),
                token_count(200, 100, 20),
            ],
        );

        assert_eq!(
            sync_test_file(&db, &child_a, &[&child_a, &child_b])?.imported,
            1
        );
        assert_eq!(
            sync_test_file(&db, &child_b, &[&child_a, &child_b])?.imported,
            1
        );

        let conn = lock_conn!(db.conn);
        let request_ids = conn
            .prepare(
                "SELECT request_id FROM proxy_request_logs
                 WHERE data_source = 'codex_session' ORDER BY request_id",
            )?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            request_ids,
            vec![
                format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_A_ID}:1"),
                format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_B_ID}:1")
            ]
        );
        Ok(())
    }

    #[test]
    fn test_archived_log_inherits_cursor_and_only_imports_appended_usage() -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let sessions = temp.path().join("sessions");
        let archived = temp.path().join("archived_sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(&archived).unwrap();
        let source = rollout_path(&sessions, PARENT_ID);
        let archived_file = rollout_path(&archived, PARENT_ID);
        write_jsonl(
            &archived_file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_count(100, 50, 10),
                token_count(200, 100, 20),
            ],
        );

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens,
                    total_cost_usd, latency_ms, status_code, session_id,
                    created_at, data_source
                ) VALUES ('codex_session:parent:2', '_codex_session', 'codex',
                          'gpt-5.6-sol', 'gpt-5.6-sol', 999, 99, 0, '0', 0,
                          200, 'parent', 1, 'codex_session')",
                [],
            )?;
        }
        let source_path = source.to_string_lossy().to_string();
        update_sync_state(&db, &source_path, 1, 3)?;

        assert_eq!(
            sync_test_file(&db, &archived_file, &[&archived_file])?.imported,
            1
        );
        assert_eq!(
            sync_test_file(&db, &archived_file, &[&archived_file])?.imported,
            0
        );

        let conn = lock_conn!(db.conn);
        let old_row_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM proxy_request_logs
             WHERE request_id = 'codex_session:parent:2'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(old_row_count, 1);
        let usage: (i64, i64, i64) = conn.query_row(
            "SELECT input_tokens, cache_read_tokens, output_tokens
             FROM proxy_request_logs
             WHERE request_id = ?1",
            [format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{PARENT_ID}:2")],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(usage, (100, 50, 10));
        drop(conn);
        assert_eq!(get_sync_state(&db, &archived_file.to_string_lossy())?.1, 4);

        Ok(())
    }

    #[test]
    fn test_insert_codex_session_keeps_approximate_match_for_read_time_dedup(
    ) -> Result<(), AppError> {
        let db = Database::memory()?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    total_cost_usd, latency_ms, status_code, session_id,
                    created_at, data_source
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    "codex-proxy",
                    "openai",
                    "codex",
                    "gpt-5.4",
                    "gpt-5.4",
                    10,
                    2,
                    1,
                    7,
                    "0.01",
                    100,
                    200,
                    "codex_session-1",
                    1000,
                    "proxy"
                ],
            )?;
        }

        let delta = DeltaTokens {
            input: 10,
            cached_input: 1,
            output: 2,
        };
        let mut suspected_duplicates = 0;
        let inserted = insert_test_codex_session_entry(
            &db,
            "codex-session-dup",
            &delta,
            "gpt-5.4",
            Some("session-1"),
            Some("1970-01-01T00:16:45Z"),
            &mut suspected_duplicates,
        )?;
        assert!(inserted);

        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
            row.get(0)
        })?;
        assert_eq!(count, 2);
        drop(conn);

        let summary = db.get_usage_summary(None, None, Some("codex"))?;
        assert_eq!(summary.total_requests, 1);

        Ok(())
    }

    #[test]
    fn test_codex_session_duplicate_is_observed_but_still_inserted() -> Result<(), AppError> {
        let db = Database::memory()?;
        let delta = DeltaTokens {
            input: 10,
            cached_input: 1,
            output: 2,
        };
        let mut suspected_duplicates = 0;
        assert!(insert_test_codex_session_entry(
            &db,
            "codex-session-a",
            &delta,
            "gpt-5.4",
            Some("session-a"),
            Some("1970-01-01T00:16:40Z"),
            &mut suspected_duplicates,
        )?);
        assert!(insert_test_codex_session_entry(
            &db,
            "codex-session-b",
            &delta,
            "gpt-5.4",
            Some("session-b"),
            Some("1970-01-01T00:16:45Z"),
            &mut suspected_duplicates,
        )?);
        assert_eq!(suspected_duplicates, 1);

        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = 'codex_session'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(count, 2);
        Ok(())
    }

    #[test]
    fn codex_rebuild_guard_only_rejects_codex_proxy_rollups() -> Result<(), AppError> {
        let db = Database::memory()?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "INSERT INTO usage_daily_rollups (date, app_type, provider_id, model)
                 VALUES
                    ('2026-07-10', 'codex', '_codex_session', 'gpt'),
                    ('2026-07-10', 'gemini', 'gemini-provider', 'gemini');",
            )?;
        }
        db.ensure_codex_usage_rebuild_safe()?;

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO usage_daily_rollups (date, app_type, provider_id, model)
                 VALUES ('2026-07-10', 'codex', 'proxy-provider', 'gpt')",
                [],
            )?;
        }
        let error = db
            .ensure_codex_usage_rebuild_safe()
            .expect_err("Codex proxy rollups make a full rebuild ambiguous");
        assert!(error
            .to_string()
            .contains("Cannot safely rebuild Codex usage"));
        Ok(())
    }

    #[test]
    fn reset_codex_usage_only_removes_codex_rows_and_structural_cursors() -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let wide_dir = temp.path();
        let current_codex = rollout_path(&wide_dir.join("sessions"), CHILD_A_ID);
        let legacy_codex =
            format!("C:\\old-codex\\archived_sessions\\rollout-old-{CHILD_B_ID}.jsonl");
        let gemini_cursor = wide_dir.join("gemini/sessions/session-123.json");
        let claude_cursor = wide_dir.join(format!("projects/rollout-{PARENT_ID}.jsonl"));

        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, input_tokens,
                    output_tokens, cache_read_tokens, latency_ms, status_code,
                    created_at, data_source
                 ) VALUES
                    ('codex-row', '_codex_session', 'codex', 'gpt', 1, 1, 0, 0, 200, 1, 'codex_session'),
                    ('gemini-row', '_gemini_session', 'gemini', 'gemini', 1, 1, 0, 0, 200, 1, 'gemini_session');
                 INSERT INTO usage_daily_rollups (date, app_type, provider_id, model)
                 VALUES
                    ('2026-07-10', 'codex', '_codex_session', 'gpt'),
                    ('2026-07-10', 'gemini', '_gemini_session', 'gemini');",
            )?;
            for path in [
                current_codex.to_string_lossy().to_string(),
                legacy_codex,
                gemini_cursor.to_string_lossy().to_string(),
                claude_cursor.to_string_lossy().to_string(),
            ] {
                conn.execute(
                    "INSERT INTO session_log_sync
                     (file_path, last_modified, last_line_offset, last_synced_at)
                     VALUES (?1, 1, 1, 1)",
                    [path],
                )?;
            }

            reset_codex_usage_on_conn(&conn, wide_dir)?;
            let codex_rows: i64 = conn.query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = 'codex_session'",
                [],
                |row| row.get(0),
            )?;
            let gemini_rows: i64 = conn.query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = 'gemini_session'",
                [],
                |row| row.get(0),
            )?;
            let codex_rollups: i64 = conn.query_row(
                "SELECT COUNT(*) FROM usage_daily_rollups WHERE provider_id = '_codex_session'",
                [],
                |row| row.get(0),
            )?;
            let remaining_cursors: i64 =
                conn.query_row("SELECT COUNT(*) FROM session_log_sync", [], |row| {
                    row.get(0)
                })?;
            assert_eq!((codex_rows, gemini_rows, codex_rollups), (0, 1, 0));
            assert_eq!(remaining_cursors, 2);
        }
        Ok(())
    }

    // ── 模型名归一化测试 ──

    #[test]
    fn test_normalize_codex_model_lowercase() {
        assert_eq!(normalize_codex_model("GLM-4.6"), "glm-4.6");
        assert_eq!(normalize_codex_model("DeepSeek-Chat"), "deepseek-chat");
        assert_eq!(normalize_codex_model("GPT-5.4"), "gpt-5.4");
    }

    #[test]
    fn test_normalize_codex_model_strip_prefix() {
        assert_eq!(normalize_codex_model("openai/gpt-5.4"), "gpt-5.4");
        assert_eq!(
            normalize_codex_model("azure/gpt-5.2-codex"),
            "gpt-5.2-codex"
        );
        assert_eq!(normalize_codex_model("OPENAI/GPT-5.4"), "gpt-5.4");
    }

    #[test]
    fn test_normalize_codex_model_strip_iso_date() {
        assert_eq!(normalize_codex_model("gpt-5.4-2026-03-05"), "gpt-5.4");
        assert_eq!(
            normalize_codex_model("gpt-5.4-pro-2026-03-05"),
            "gpt-5.4-pro"
        );
    }

    #[test]
    fn test_normalize_codex_model_strip_compact_date() {
        assert_eq!(normalize_codex_model("gpt-5.4-20260305"), "gpt-5.4");
        assert_eq!(
            normalize_codex_model("claude-opus-4-6-20260206"),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn test_normalize_codex_model_no_change() {
        assert_eq!(normalize_codex_model("gpt-5.4"), "gpt-5.4");
        assert_eq!(normalize_codex_model("gpt-5.2-codex"), "gpt-5.2-codex");
        assert_eq!(normalize_codex_model("o3"), "o3");
        assert_eq!(normalize_codex_model("deepseek-chat"), "deepseek-chat");
    }

    #[test]
    fn test_normalize_codex_model_combined() {
        // prefix + uppercase + ISO date
        assert_eq!(
            normalize_codex_model("openai/GPT-5.4-2026-03-05"),
            "gpt-5.4"
        );
        // prefix + compact date
        assert_eq!(normalize_codex_model("openai/gpt-5.4-20260305"), "gpt-5.4");
    }

    #[test]
    fn test_cached_clamped_to_input() {
        // cached > input 的异常场景应被 min() 钳制
        let prev = Some(CumulativeTokens {
            input: 100,
            cached_input: 0,
            output: 50,
        });
        let current = CumulativeTokens {
            input: 110,       // delta = 10
            cached_input: 80, // delta = 80（异常：大于 input delta）
            output: 60,
        };
        let delta = compute_delta(&prev, &current);
        // 钳制前：cached_input = 80, input = 10
        assert_eq!(delta.cached_input, 80);
        assert_eq!(delta.input, 10);
        // 实际钳制在调用侧：delta.cached_input.min(delta.input)
        let clamped = delta.cached_input.min(delta.input);
        assert_eq!(clamped, 10);
    }
}
