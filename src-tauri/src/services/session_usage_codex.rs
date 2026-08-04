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
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime};

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

/// 父 rollout 快照的新鲜度戳
///
/// 由**已打开句柄**的 `fstat` 生成（见 `parent_signatures_before`）：戳与随后读到
/// 的内容必然出自同一个文件对象，不存在"stat 到 A、读到 B"的窗口。
///
/// Unix 上除 `(mtime_ns, size)` 外还带 `(dev, ino)`：symlink 改指向、或把另一个
/// 尺寸与 mtime 都相同的文件 rename 覆盖上来时，前两项可能完全一致，只有 inode
/// 会变。
///
/// 已知且接受的残余风险：
/// - 同 inode、同尺寸、同 `mtime_ns` 的**原地改写**检测不到。rollout 是追加写的，
///   这种改写只可能来自人为篡改。
/// - 非 Unix 上退化为 `(mtime_ns, size)`：一次保持时间戳不变的同尺寸原子替换
///   （先写好新文件再 rename 覆盖）对缓存不可见。std 的 `File::file_index()` 目前
///   仍是 unstable，为此单独引入 Windows 平台依赖超出本次范围；上游移植时建议改用
///   `GetFileInformationByHandle` 取 `(dwVolumeSerialNumber, nFileIndexHigh/Low)`
///   把这一项补齐。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ParentFileStamp {
    modified_nanos: i64,
    size: u64,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

impl ParentFileStamp {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                modified_nanos: metadata_modified_nanos(metadata),
                size: metadata.len(),
                dev: metadata.dev(),
                ino: metadata.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                modified_nanos: metadata_modified_nanos(metadata),
                size: metadata.len(),
            }
        }
    }
}

/// 父 rollout 的整文件签名快照
///
/// 与上游 df3e07ed 的差异（纯性能优化）：上游把缓存键定为
/// `(parent_path, cutoff_micros)`，而每个 fork 子线程的 cutoff 各不相同，
/// 于是同一个父文件会被完整重解析 N 次（N = 子线程数）。这里改为按文件缓存
/// 整表——保存全部有序 `(timestamp, signature)` 对与文件级 `max_timestamp`，
/// 用 `ParentFileStamp` 做新鲜度校验，查询时在内存里按 cutoff 过滤。
///
/// 跨调用行为比上游更新鲜而非更陈旧：上游的 `(path, cutoff)` 缓存在文件变化时
/// 从不失效，且把 cutoff 截断到微秒；这里父文件一被写入戳就会变、触发重解析，
/// cutoff 也保持纳秒精度。所有 Ok/Err 判定与错误文案与上游一致，唯一的声明性
/// 语义差异见 `parse_parent_signature_file` 的文档注释。
#[derive(Debug, Clone)]
struct ParentSignatureSnapshot {
    /// 新鲜度戳
    stamp: ParentFileStamp,
    /// 按文件顺序保存的 token_count 签名及其时间戳
    entries: Vec<(DateTime<Utc>, TokenUsageSignature)>,
    /// 全文件（含非 token_count 行）的最大 timestamp
    max_timestamp: Option<DateTime<Utc>>,
    /// 是否存在缺少有效 timestamp 的 token_count 行（上游扫描到该行即报错）
    missing_timestamp: bool,
}

#[derive(Debug, Default)]
struct CodexReplayCaches {
    parent_signatures: HashMap<PathBuf, ParentSignatureSnapshot>,
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

/// 解析 rollout 时间戳字符串（RFC3339）
///
/// 抽出独立函数以便 `parse_timestamp`（`Value` 路径）与父文件扫描的窄化解析器
/// （`ParentLine`）共用同一套解析规则：两侧只接受字符串，且接受完全相同的
/// RFC3339 写法。
fn parse_timestamp_str(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn parse_timestamp(value: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    value
        .and_then(serde_json::Value::as_str)
        .and_then(parse_timestamp_str)
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

// ---------------------------------------------------------------------------
// 父 rollout 行的窄化解析（`ParentLine` / `ParentPayload`）
//
// 父文件扫描只关心四个值：顶层 `timestamp`、顶层 `type`、`payload.type`、
// `payload.info`。这里手写 visitor 只捕获它们，其余键的值一律交给 `IgnoredAny`
// 跳过——不构造 `Value` 树、不为长正文分配 `String`。
//
// 刻意不用 `#[derive(Deserialize)]`：derive 生成的结构体反序列化遇到重复字段会
// 直接报错，而参考实现用的是开启 `preserve_order` 的 `serde_json::Value`，重复键
// **取最后一个**。下面的 visitor 显式实现「后者覆盖前者」。
// ---------------------------------------------------------------------------

/// 捕获「字符串值 / 非字符串值」的字段
///
/// 参考实现读字段走的是 `Value::get(..).and_then(Value::as_str)`：非字符串
/// （数字、null、对象、数组）一律等价于「没有这个字符串」，而不是解析失败。
/// 这里保持同一语义——非字符串值原样跳过并返回 `None`。
///
/// 字符串本身无转义时零拷贝借用输入切片（`visit_borrowed_str`），有转义时回落
/// `Cow::Owned`（`visit_str` / `visit_string`），于是 `"\u0074oken_count"` 与字面
/// `"token_count"` 完全等价——这正是被删掉的原始字节前置过滤漏掉的那一类。
struct MaybeStr<'de>(Option<Cow<'de, str>>);

struct MaybeStrVisitor;

impl<'de> Visitor<'de> for MaybeStrVisitor {
    type Value = MaybeStr<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("任意 JSON 值")
    }

    fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E> {
        Ok(MaybeStr(Some(Cow::Borrowed(value))))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(MaybeStr(Some(Cow::Owned(value.to_owned()))))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(MaybeStr(Some(Cow::Owned(value))))
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(MaybeStr(None))
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(MaybeStr(None))
    }

    fn visit_i128<E>(self, _value: i128) -> Result<Self::Value, E> {
        Ok(MaybeStr(None))
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(MaybeStr(None))
    }

    fn visit_u128<E>(self, _value: u128) -> Result<Self::Value, E> {
        Ok(MaybeStr(None))
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(MaybeStr(None))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(MaybeStr(None))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(MaybeStr(None))
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(MaybeStr(None))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(MaybeStr(None))
    }
}

impl<'de> Deserialize<'de> for MaybeStr<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(MaybeStrVisitor)
    }
}

/// 顶层键：只区分关心的三个，其余归 `Other`（键名零分配比较）
enum ParentLineField {
    Timestamp,
    Type,
    Payload,
    Other,
}

/// `payload` 内的键：只区分 `type` / `info`
enum ParentPayloadField {
    Type,
    Info,
    Other,
}

macro_rules! impl_field_key {
    ($ty:ident, $expecting:literal, { $($name:literal => $variant:ident),+ $(,)? }) => {
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct FieldVisitor;

                impl Visitor<'_> for FieldVisitor {
                    type Value = $ty;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str($expecting)
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                        Ok(match value {
                            $($name => $ty::$variant,)+
                            _ => $ty::Other,
                        })
                    }
                }

                deserializer.deserialize_str(FieldVisitor)
            }
        }
    };
}

impl_field_key!(ParentLineField, "父 rollout 行的顶层键", {
    "timestamp" => Timestamp,
    "type" => Type,
    "payload" => Payload,
});

impl_field_key!(ParentPayloadField, "父 rollout 行的 payload 键", {
    "type" => Type,
    "info" => Info,
});

/// 父 rollout 行的 `payload` 子树
///
/// `info` 无条件捕获为完整 `Value`：它可能出现在 `type` 之前，且体量很小
/// （只有 token 计数器）；捕获成 `Value` 才能原样复用既有的
/// `parse_token_signature(&Value)`，语义与参考实现逐字一致。
#[derive(Default)]
struct ParentPayload<'de> {
    kind: Option<Cow<'de, str>>,
    info: Option<serde_json::Value>,
}

struct ParentPayloadVisitor;

impl<'de> Visitor<'de> for ParentPayloadVisitor {
    type Value = ParentPayload<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("任意 JSON 值")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut payload = ParentPayload::default();
        while let Some(field) = map.next_key::<ParentPayloadField>()? {
            match field {
                // 重复键取最后一个：无条件覆盖，与 `preserve_order` 的 Value 一致。
                ParentPayloadField::Type => {
                    payload.kind = map.next_value::<MaybeStr<'de>>()?.0;
                }
                ParentPayloadField::Info => {
                    payload.info = Some(map.next_value::<serde_json::Value>()?);
                }
                ParentPayloadField::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(payload)
    }

    // payload 不是对象时（字符串/数字/数组/null），参考实现的 `payload.get("type")`
    // 只会得到 None，而不是整行解析失败——这里同样原样跳过并返回空 payload。
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(ParentPayload::default())
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(ParentPayload::default())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(ParentPayload::default())
    }

    fn visit_i128<E>(self, _value: i128) -> Result<Self::Value, E> {
        Ok(ParentPayload::default())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(ParentPayload::default())
    }

    fn visit_u128<E>(self, _value: u128) -> Result<Self::Value, E> {
        Ok(ParentPayload::default())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(ParentPayload::default())
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(ParentPayload::default())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(ParentPayload::default())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(ParentPayload::default())
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Deserialize<'de> for ParentPayload<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ParentPayloadVisitor)
    }
}

/// 父 rollout 的一行
#[derive(Default)]
struct ParentLine<'de> {
    timestamp: Option<Cow<'de, str>>,
    kind: Option<Cow<'de, str>>,
    payload: Option<ParentPayload<'de>>,
}

struct ParentLineVisitor;

impl<'de> Visitor<'de> for ParentLineVisitor {
    type Value = ParentLine<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("任意 JSON 值")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut line = ParentLine::default();
        while let Some(field) = map.next_key::<ParentLineField>()? {
            match field {
                // 重复键取最后一个：无条件覆盖。`{"timestamp":"A","timestamp":123}`
                // 因此会把 timestamp 覆写回 None，与 Value 里 `as_str()` 的结果一致。
                ParentLineField::Timestamp => {
                    line.timestamp = map.next_value::<MaybeStr<'de>>()?.0;
                }
                ParentLineField::Type => {
                    line.kind = map.next_value::<MaybeStr<'de>>()?.0;
                }
                ParentLineField::Payload => {
                    line.payload = Some(map.next_value::<ParentPayload<'de>>()?);
                }
                ParentLineField::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(line)
    }

    // 顶层不是对象时（数组/字符串/数字…），参考实现的 `value.get(..)` 全是 None，
    // 该行既不贡献 max_timestamp 也不贡献签名——这里同样跳过后返回空行。
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(ParentLine::default())
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(ParentLine::default())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(ParentLine::default())
    }

    fn visit_i128<E>(self, _value: i128) -> Result<Self::Value, E> {
        Ok(ParentLine::default())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(ParentLine::default())
    }

    fn visit_u128<E>(self, _value: u128) -> Result<Self::Value, E> {
        Ok(ParentLine::default())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(ParentLine::default())
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(ParentLine::default())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(ParentLine::default())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(ParentLine::default())
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Deserialize<'de> for ParentLine<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ParentLineVisitor)
    }
}

impl ParentSignatureSnapshot {
    /// 在内存快照上回答某个 cutoff 的查询（Ok/Err 判定与上游逐行扫描一致）
    fn query(
        &self,
        parent_path: &Path,
        cutoff: DateTime<Utc>,
    ) -> Result<Vec<TokenUsageSignature>, String> {
        // 上游在扫描到缺时间戳的 token_count 行时立即返回，故该错误优先于下面的
        // “尚未写到 child fork 时刻”。
        if self.missing_timestamp {
            return Err(format!(
                "父 rollout {} 的 token_count 缺少有效 timestamp",
                parent_path.display()
            ));
        }
        if self
            .max_timestamp
            .is_none_or(|timestamp| timestamp < cutoff)
        {
            return Err(format!(
                "父 rollout {} 尚未写到 child fork 时刻",
                parent_path.display()
            ));
        }
        Ok(self
            .entries
            .iter()
            .filter(|(timestamp, _)| *timestamp <= cutoff)
            .map(|(_, signature)| signature.clone())
            .collect())
    }
}

/// 整表解析父 rollout：收集全部 token_count 签名、文件级最大时间戳
///
/// ## 解析策略：一条路径，不猜字节
///
/// 每一行都只走 `serde_json::from_str::<ParentLine>` 这一条路径——一个只捕获
/// `timestamp` / `type` / `payload.type` / `payload.info` 的窄化 visitor，其余键
/// 的值交给 `IgnoredAny` 跳过。父文件扫描里不存在任何"用原始字节猜 JSON 语义"的
/// 前置过滤或快路径，因此转义写法（判别字符串写成 `"\u0074oken_count"` /
/// `"\u0065vent_msg"`）、带空白的重复键（`"timestamp" : "B"`）、转义的重复键名
/// （`"\u0074imestamp"`）等全部由解析器按 JSON 规范处理，与参考实现（逐行构造
/// `serde_json::Value`）结论一致。重复键取最后一个，对齐本 crate 开启
/// `preserve_order` 的 `Value`。
///
/// ## 与参考实现的声明性语义差异
///
/// 除下面两类行外，签名（`entries`）、错误判定（`missing_timestamp` /
/// "尚未写到"）与错误文案对**任意输入**都与参考实现逐字一致。
///
/// ### 差异一：`Value` 比"只做跳过"的解析严格
///
/// 这类行**语法合法、但 `serde_json::Value` 构造不出来**。`Value` 额外拒绝三种
/// 写法：
/// - 浮点溢出（如 `1e400`：`Value` 只接受有限数）；
/// - 嵌套深度 > 127（`Value` 有 128 层递归上限；`IgnoredAny` 的跳过是迭代式的、
///   不设上限）；
/// - 孤立代理项转义（如 `"\uD800"`：`Value` 要求可解码成 `String`）。
///
/// 参考实现对这类行整行丢弃（`from_str::<Value>` 直接报错），本实现则照常处理：
/// 顶层 `timestamp` 会计入 `max_timestamp`；若该行同时是合法的 token_count 事件，
/// 其签名也会被收下（缺 timestamp 时同样会置 `missing_timestamp`）。这是**刻意
/// 选择**：能解析出来的顶层 timestamp 就是"文件已经写到这里"的证据，与某个无关
/// 子树里有没有垃圾无关；参考实现的整行丢弃只是"拿 Value 当解析器"的副作用。
///
/// 边界按**解析路径**划分，而不是按"键名 / 值"划分：
/// - **落在解析路径上**的位置与 `Value` 同样严格：顶层键名、`payload` 层键名、
///   被捕获的 `timestamp` / `type` 字符串值、被整棵捕获的 `info` 子树。这些位置
///   上的垃圾同样让本解析器整行失败 → 两个实现一起跳过整行，零差异。
/// - **落在被跳过子树内部**的位置走 `IgnoredAny` 的宽松跳过，属于本差异类；
///   被跳过的子树是：顶层 `timestamp`/`type`/`payload` 之外的键的值、`payload`
///   里 `type`/`info` 之外的键的值。注意其中的**键名与值一样宽松**：
///   `"junk":{"\uD800":0}` 的孤立代理项虽然在键名上，但那是被跳过子树里的嵌套
///   键名，`Value` 拒绝而本解析器接受，仍属本差异类。
/// - 现实中没有序列化器会产出这类行；写入截断产生的是**语法非法**行，两个实现
///   都会跳过（见 `test_parent_signatures_ignore_invalid_json_max_timestamp`）。
///
/// ### 差异二：不复刻 serde_json 的 `RawValue` 私有哨兵
///
/// 本解析器按 JSON 规范读普通 JSON，**不实现** serde_json 的私有哨兵
/// `{"$serde_json::private::RawValue":"<被转义的 JSON 文本>"}`。该哨兵在本 crate
/// 里是活的：依赖图（axum）传递性打开了 serde_json 的 `raw_value` feature，于是
/// `from_str::<Value>` 遇到这种单键对象时，会把内层字符串当 JSON **就地展开**，
/// 把它重新解释成所嵌的对象；本解析器则只把 `$serde_json::private::RawValue`
/// 当作一个普通的未知键跳过。
///
/// 可观察后果（如实陈述）：对手工构造的这类行，参考实现可能解出本解析器根本读不到
/// 的 timestamp / 判别串 / 签名，两侧因此发散。哨兵出现在顶层对象、`timestamp` /
/// `type`、`payload` 等**任何**位置都可能触发；唯独落在被整棵捕获的 `info` 子树
/// 内部时两侧都交给 `Value` 处理，反而一致。
///
/// 不复刻的理由：没有任何序列化器会写出这个私有哨兵（它只在 serde_json 内部
/// `RawValue` 的序列化/反序列化握手中出现），而复刻一个可能随 serde_json 版本
/// 变化的私有实现细节，比把它声明出来更糟。
fn parse_parent_signature_file(
    file: fs::File,
    parent_path: &Path,
    stamp: ParentFileStamp,
) -> ParentSignatureSnapshot {
    let started = Instant::now();
    let mut entries: Vec<(DateTime<Utc>, TokenUsageSignature)> = Vec::new();
    let mut max_timestamp: Option<DateTime<Utc>> = None;
    let mut missing_timestamp = false;
    let mut total_lines = 0u64;

    // 必须扫描完整父文件并逐行应用 cutoff，不能在首个未来时间戳处 break：
    // rollout 写入顺序不承诺时间戳严格单调。
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else {
            continue;
        };
        total_lines += 1;

        // 解析失败即跳过该行（与参考实现对非法 JSON 的处理一致）。
        let Ok(parsed) = serde_json::from_str::<ParentLine<'_>>(&line) else {
            continue;
        };
        let timestamp = parsed.timestamp.as_deref().and_then(parse_timestamp_str);
        if let Some(timestamp) = timestamp {
            max_timestamp = Some(max_timestamp.map_or(timestamp, |current| current.max(timestamp)));
        }
        if parsed.kind.as_deref() != Some("event_msg") {
            continue;
        }
        let Some(payload) = parsed.payload else {
            continue;
        };
        if payload.kind.as_deref() != Some("token_count") {
            continue;
        }
        let Some(info) = payload.info.filter(|info| !info.is_null()) else {
            continue;
        };
        let Some(signature) = parse_token_signature(&info) else {
            continue;
        };
        let Some(timestamp) = timestamp else {
            missing_timestamp = true;
            continue;
        };
        entries.push((timestamp, signature));
    }

    log::debug!(
        "[CODEX-SYNC] parent-cache miss {}: lines={total_lines} signatures={} missing_ts={missing_timestamp} 耗时 {:?}",
        parent_path.display(),
        entries.len(),
        started.elapsed()
    );

    ParentSignatureSnapshot {
        stamp,
        entries,
        max_timestamp,
        missing_timestamp,
    }
}

/// 查询父 rollout 在 cutoff 之前的全部 token_count 签名
///
/// ## 先 open、再 fstat、再读，全程同一个句柄
///
/// 1. **无条件先 `open`**。打开失败直接返回，错误文案与参考实现同一份，因此
///    `open` 类错误永远是**当次**的新鲜结果，不会被缓存掩盖。这一点是必要的：
///    `chmod 000` 只改 ctime/mode，`(mtime_ns, size, dev, ino)` 全不变，若先查缓存
///    就会把旧的内容型错误（"尚未写到…"）当成答案，与参考实现逐字发散。缓存能够
///    回答的只有**由内容推导**的结论（签名集合、"尚未写到"、"缺少有效 timestamp"）。
/// 2. 新鲜度戳取自这个句柄的 `fstat`（而非再对路径 stat 一次），戳与随后读到的
///    内容必然出自同一个文件对象，不存在"stat 到 A、读到 B"的窗口。
/// 3. 戳与缓存快照一致就丢掉句柄、直接在内存快照上按 cutoff 作答；不一致或未命中
///    则从**同一个句柄**整表重解析，写回缓存后作答。
///
/// 父文件在解析期间被继续追加时，快照带的仍是解析开始时的戳，下次查询自然重解析。
/// 残余风险见 `ParentFileStamp` 的文档注释。
fn parent_signatures_before(
    parent_path: &Path,
    cutoff: DateTime<Utc>,
) -> Result<Vec<TokenUsageSignature>, String> {
    let file = fs::File::open(parent_path)
        .map_err(|error| format!("无法打开父 rollout {}: {error}", parent_path.display()))?;
    let stamp = file
        .metadata()
        .ok()
        .map(|metadata| ParentFileStamp::from_metadata(&metadata));

    if let Some(stamp) = stamp {
        if let Ok(caches) = replay_caches().lock() {
            if let Some(snapshot) = caches.parent_signatures.get(parent_path) {
                if snapshot.stamp == stamp {
                    log::debug!(
                        "[CODEX-SYNC] parent-cache hit {}: signatures={} cutoff={cutoff}",
                        parent_path.display(),
                        snapshot.entries.len()
                    );
                    // 缓存有效：句柄再无用处，先释放再作答。
                    drop(file);
                    return snapshot.query(parent_path, cutoff);
                }
            }
        }
    }

    let snapshot = parse_parent_signature_file(file, parent_path, stamp.unwrap_or_default());
    let answer = snapshot.query(parent_path, cutoff);
    if stamp.is_some() {
        if let Ok(mut caches) = replay_caches().lock() {
            caches
                .parent_signatures
                .insert(parent_path.to_path_buf(), snapshot);
        }
    }
    answer
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
    let pricing_model = if pricing.is_some() { model } else { "" };
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
            request_id, provider_id, app_type, model, request_model, pricing_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at, data_source
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
        )
        .map_err(|e| AppError::Database(format!("插入 Codex 会话日志失败: {e}")))?;
    let inserted_rows = stmt
        .execute(rusqlite::params![
            request_id,
            "_codex_session", // provider_id
            "codex",          // app_type
            model,
            model, // request_model = model
            pricing_model,
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
    fn codex_session_import_records_write_time_pricing_evidence() -> Result<(), AppError> {
        let db = Database::memory()?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT OR REPLACE INTO model_pricing (
                     model_id, display_name, input_cost_per_million,
                     output_cost_per_million, cache_read_cost_per_million,
                     cache_creation_cost_per_million
                 ) VALUES ('codex-writer-free', 'Codex Writer Free', '0', '0', '0', '0')",
                [],
            )?;
        }
        let delta = DeltaTokens {
            input: 10,
            cached_input: 0,
            output: 1,
        };
        let mut suspected_duplicates = 0;
        assert!(insert_test_codex_session_entry(
            &db,
            "codex-priced-evidence",
            &delta,
            "codex-writer-free",
            Some("codex-priced-evidence"),
            Some("1970-01-01T00:16:40Z"),
            &mut suspected_duplicates,
        )?);

        let conn = lock_conn!(db.conn);
        let pricing_model: Option<String> = conn.query_row(
            "SELECT pricing_model FROM proxy_request_logs
             WHERE request_id = 'codex-priced-evidence'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(pricing_model.as_deref(), Some("codex-writer-free"));

        Ok(())
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
    fn test_insert_codex_session_skips_matching_proxy_log() -> Result<(), AppError> {
        let db = Database::memory()?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    total_cost_usd, latency_ms, status_code, created_at, data_source
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        assert!(!inserted);

        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
            row.get(0)
        })?;
        assert_eq!(count, 1);

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

    // ---------------------------------------------------------------------
    // parent_signatures_before 等价性 oracle
    //
    // `parent_signatures_before_reference` 是上游 df3e07ed 的原始实现（逐行
    // 完整 JSON 解析、无文件级缓存）。下面的测试用合成 rollout 语料证明
    // 新实现（按文件缓存 + 单一窄化 visitor 解析）输出逐字一致。
    // ---------------------------------------------------------------------

    fn parent_signatures_before_reference(
        parent_path: &Path,
        cutoff: DateTime<Utc>,
    ) -> Result<Vec<TokenUsageSignature>, String> {
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
                max_timestamp =
                    Some(max_timestamp.map_or(timestamp, |current| current.max(timestamp)));
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

        Ok(signatures)
    }

    /// 确定性伪随机（LCG），避免引入 rand 依赖
    struct Lcg(u64);

    impl Lcg {
        fn new(seed: u64) -> Self {
            Self(seed.wrapping_mul(6364136223846793005).wrapping_add(1))
        }

        fn next_u64(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }

        fn below(&mut self, bound: u64) -> u64 {
            self.next_u64() % bound.max(1)
        }
    }

    fn corpus_base() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-10T03:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn corpus_time(offset_secs: u64) -> DateTime<Utc> {
        corpus_base() + chrono::Duration::seconds(offset_secs as i64)
    }

    fn corpus_ts(offset_secs: u64) -> String {
        corpus_time(offset_secs).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    /// 同一时刻的 `+08:00` 写法，用于校验 RFC3339 变体两侧解析一致
    fn corpus_ts_offset(offset_secs: u64) -> String {
        let zone = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        corpus_time(offset_secs).with_timezone(&zone).to_rfc3339()
    }

    fn token_count_line(ts: &str, index: u64) -> String {
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{},"reasoning_output_tokens":{},"total_tokens":{}}},"last_token_usage":{{"input_tokens":{},"output_tokens":{}}}}}}}}}"#,
            1000 + index * 7,
            index * 3,
            200 + index,
            index,
            1200 + index * 8,
            index + 1,
            index + 2,
        )
    }

    fn long_content(index: u64, filler: usize) -> String {
        // 正文里塞入被转义的 `\"timestamp\":\"...\"`：合法 JSON 中字符串内的引号
        // 必然转义，解析器不应把正文内容误当作重复键。
        format!(
            "assistant text {index} {} 引用了 \\\"timestamp\\\":\\\"2030-01-01T00:00:00Z\\\" 结束",
            "x".repeat(filler)
        )
    }

    /// 语料分布
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CorpusProfile {
        /// 对抗性：各类边界行高频出现，用于等价性 oracle
        Adversarial,
        /// 贴近真实 rollout：约 2% token_count，其余绝大多数是长正文行
        Realistic,
    }

    impl CorpusProfile {
        /// 把一次随机采样映射到 `synthetic_rollout_corpus` 的行类型编号
        fn kind(self, rng: &mut Lcg) -> u64 {
            match self {
                CorpusProfile::Adversarial => rng.below(100),
                CorpusProfile::Realistic => match rng.below(1000) {
                    0..=19 => 0,     // 2%   有效 token_count
                    20..=24 => 20,   // 0.5% token_count info=null
                    25..=29 => 24,   // 0.5% 正文含 token_count 字面量
                    30..=959 => 28,  // 93%  长正文行（正文体量的主力）
                    960..=979 => 58, // 2%   turn_context
                    980..=984 => 68, // 0.5% 字段顺序不同
                    985..=989 => 72, // 0.5% 带时区偏移
                    990..=994 => 76, // 0.5% 截断的非法 JSON
                    995..=996 => 84, // 0.2% 缺 timestamp
                    997..=998 => 90, // 0.2% 重复 timestamp 键
                    _ => 96,         // 0.1% 空行
                },
            }
        }
    }

    /// 生成一份混合了各种边界行的合成 rollout 语料
    ///
    /// 返回 (文件内容, 可用作 cutoff 的时间戳候选)
    fn synthetic_rollout_corpus(
        seed: u64,
        line_count: usize,
        filler: usize,
        profile: CorpusProfile,
    ) -> (String, Vec<DateTime<Utc>>) {
        let mut rng = Lcg::new(seed);
        let mut slots: Vec<u64> = (1..=line_count as u64).collect();
        // 打乱时间戳顺序：rollout 不承诺时间戳单调
        for index in (1..slots.len()).rev() {
            let target = rng.below((index + 1) as u64) as usize;
            slots.swap(index, target);
        }

        let mut lines: Vec<String> = Vec::with_capacity(line_count);
        let mut candidates: Vec<DateTime<Utc>> = Vec::new();

        for (index, slot) in slots.iter().copied().enumerate() {
            let index = index as u64;
            let ts = corpus_ts(slot);
            let kind = profile.kind(&mut rng);
            let line = match kind {
                0..=19 => {
                    // 有效 token_count
                    candidates.push(corpus_time(slot));
                    token_count_line(&ts, index)
                }
                20..=23 => {
                    // token_count 但 info 为 null（无签名）
                    candidates.push(corpus_time(slot));
                    format!(
                        r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":null}}}}"#
                    )
                }
                24..=27 => {
                    // 正文里出现 token_count 字面量（不是真的 token_count 事件）
                    format!(
                        r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"agent_message","message":"讨论 \"token_count\" 事件：{}"}}}}"#,
                        long_content(index, filler)
                    )
                }
                28..=57 => {
                    // 普通长正文行
                    format!(
                        r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"agent_message","message":"{}"}}}}"#,
                        long_content(index, filler)
                    )
                }
                58..=63 => {
                    format!(
                        r#"{{"timestamp":"{ts}","type":"turn_context","payload":{{"model":"gpt-5.6-sol"}}}}"#
                    )
                }
                64..=67 => {
                    format!(
                        r#"{{"timestamp":"{ts}","type":"session_meta","payload":{{"id":"{PARENT_ID}","source":"cli"}}}}"#
                    )
                }
                68..=71 => {
                    // 字段顺序不同：timestamp 不在行首
                    format!(
                        r#"{{"type":"turn_context","timestamp":"{ts}","payload":{{"model":"gpt-5.6-sol"}}}}"#
                    )
                }
                72..=75 => {
                    // RFC3339 带时区偏移写法
                    format!(
                        r#"{{"timestamp":"{}","type":"turn_context","payload":{{}}}}"#,
                        corpus_ts_offset(slot)
                    )
                }
                76..=79 => {
                    // 非法 JSON，但行首是合法 timestamp 前缀（截断写入）
                    format!(
                        r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"agent_message","message":"truncated {}"#,
                        long_content(index, filler / 4)
                    )
                }
                80..=83 => "not json at all {\"timestamp\":\"2031-01-01T00:00:00Z\"".to_string(),
                84..=86 => {
                    // 缺 timestamp 字段
                    r#"{"type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#.to_string()
                }
                87..=89 => {
                    // timestamp 非法
                    r#"{"timestamp":"not-a-timestamp","type":"turn_context","payload":{}}"#
                        .to_string()
                }
                90..=92 => {
                    // 重复 timestamp 键：Value(preserve_order) 取后者
                    format!(
                        r#"{{"timestamp":"{ts}","type":"turn_context","timestamp":"{}","payload":{{}}}}"#,
                        corpus_ts(slot.saturating_add(line_count as u64 + 10))
                    )
                }
                93..=95 => {
                    // timestamp 为非字符串
                    format!(r#"{{"timestamp":{slot},"type":"turn_context","payload":{{}}}}"#)
                }
                _ => String::new(), // 空行
            };
            lines.push(line);
        }

        candidates.sort_unstable();
        candidates.dedup();
        (lines.join("\n") + "\n", candidates)
    }

    fn assert_oracle_matches(path: &Path, cutoffs: &[DateTime<Utc>]) {
        for cutoff in cutoffs {
            let expected = parent_signatures_before_reference(path, *cutoff);
            let actual = parent_signatures_before(path, *cutoff);
            assert_eq!(
                actual,
                expected,
                "cutoff {cutoff} 在 {} 上与参考实现不一致",
                path.display()
            );
        }
    }

    fn cutoff_matrix(candidates: &[DateTime<Utc>]) -> Vec<DateTime<Utc>> {
        let mut cutoffs = vec![
            corpus_time(0) - chrono::Duration::days(3650), // 远早于全部
            corpus_time(0),                                // 首行之前
        ];
        // 精确边界：逐个 token_count 时间戳
        cutoffs.extend(candidates.iter().copied());
        // 边界 ±1ms（落在两行之间）
        for candidate in candidates {
            cutoffs.push(*candidate - chrono::Duration::milliseconds(1));
            cutoffs.push(*candidate + chrono::Duration::milliseconds(1));
        }
        if let Some(last) = candidates.last() {
            cutoffs.push(*last + chrono::Duration::days(3650)); // 远晚于全部
        }
        cutoffs
    }

    #[test]
    fn test_parent_signatures_match_reference_on_synthetic_corpus() {
        let dir = tempdir().unwrap();

        for profile in [CorpusProfile::Adversarial, CorpusProfile::Realistic] {
            for seed in 1..=6u64 {
                let (contents, candidates) = synthetic_rollout_corpus(seed, 240, 96, profile);
                let path = dir
                    .path()
                    .join(format!("rollout-oracle-{profile:?}-{seed}.jsonl"));
                fs::write(&path, &contents).unwrap();

                assert!(
                    !candidates.is_empty(),
                    "语料应至少包含一个 token_count 时间戳"
                );
                assert_oracle_matches(&path, &cutoff_matrix(&candidates));
            }
        }
    }

    #[test]
    fn test_parent_signatures_match_reference_on_missing_timestamp() {
        let dir = tempdir().unwrap();

        // token_count 行缺 timestamp：无论 cutoff 如何都应报“缺少有效 timestamp”，
        // 且优先于“尚未写到 child fork 时刻”。
        let missing = dir.path().join("rollout-missing-ts.jsonl");
        fs::write(
            &missing,
            format!(
                "{}\n{}\n{}\n",
                token_count_line(&corpus_ts(1), 1),
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5,"output_tokens":6}}}}"#,
                token_count_line(&corpus_ts(3), 2),
            ),
        )
        .unwrap();
        assert_oracle_matches(
            &missing,
            &[
                corpus_time(0),
                corpus_time(1),
                corpus_time(2),
                corpus_time(3),
                corpus_time(9_000),
            ],
        );

        // token_count 行 timestamp 非法，同样落到“缺少有效 timestamp”
        let invalid = dir.path().join("rollout-invalid-ts.jsonl");
        fs::write(
            &invalid,
            format!(
                "{}\n{}\n",
                token_count_line("not-a-timestamp", 1),
                token_count_line(&corpus_ts(2), 2),
            ),
        )
        .unwrap();
        assert_oracle_matches(&invalid, &[corpus_time(1), corpus_time(2), corpus_time(5)]);
    }

    #[test]
    fn test_parent_signatures_ignore_invalid_json_max_timestamp() {
        // 非法 JSON 行即使带着最大 timestamp，也不能参与 max_timestamp 判定：
        // 否则“父 rollout 尚未写到 child fork 时刻”会被错误地判成 Ok。
        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout-truncated-tail.jsonl");
        fs::write(
            &path,
            format!(
                "{}\n{}\n{}\n",
                token_count_line(&corpus_ts(1), 1),
                token_count_line(&corpus_ts(2), 2),
                // 截断的未完成行，时间戳比所有完整行都晚
                format_args!(
                    r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"agent_message","message":"trunc"#,
                    corpus_ts(9_000)
                ),
            ),
        )
        .unwrap();

        let cutoff = corpus_time(5);
        let expected = parent_signatures_before_reference(&path, cutoff);
        assert!(
            expected.is_err(),
            "参考实现应认为父文件尚未写到 cutoff：{expected:?}"
        );
        assert_eq!(parent_signatures_before(&path, cutoff), expected);
        assert_oracle_matches(&path, &[corpus_time(1), corpus_time(2), corpus_time(9_000)]);
    }

    #[test]
    fn test_parent_signatures_cache_invalidated_by_append() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout-appended.jsonl");
        fs::write(
            &path,
            format!(
                "{}\n{}\n",
                token_count_line(&corpus_ts(1), 1),
                token_count_line(&corpus_ts(2), 2),
            ),
        )
        .unwrap();

        // 首次查询：命中 miss 分支并写入缓存
        let first = parent_signatures_before(&path, corpus_time(2)).unwrap();
        assert_eq!(first.len(), 2);
        // 缓存命中路径同样要复现“尚未写到 child fork 时刻”
        let ahead = parent_signatures_before(&path, corpus_time(9_000));
        assert_eq!(
            ahead,
            parent_signatures_before_reference(&path, corpus_time(9_000))
        );
        assert!(ahead.is_err());

        // 追加新行：新鲜度戳变化必须使缓存失效
        let appended = format!(
            "{}{}\n{}\n",
            fs::read_to_string(&path).unwrap(),
            token_count_line(&corpus_ts(9_000), 3),
            token_count_line(&corpus_ts(9_001), 4),
        );
        fs::write(&path, appended).unwrap();

        let after = parent_signatures_before(&path, corpus_time(9_000)).unwrap();
        assert_eq!(
            after,
            parent_signatures_before_reference(&path, corpus_time(9_000)).unwrap()
        );
        assert_eq!(after.len(), 3, "追加后应看到第 3 条签名");
        assert_oracle_matches(
            &path,
            &[
                corpus_time(1),
                corpus_time(2),
                corpus_time(9_000),
                corpus_time(9_001),
            ],
        );
    }

    // ---------------------------------------------------------------------
    // 评审反例：曾被"用原始字节猜 JSON 语义"漏掉的写法，现在必须与参考实现一致
    // ---------------------------------------------------------------------

    #[test]
    fn test_parent_signatures_match_reference_on_escaped_discriminators() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout-escaped-discriminators.jsonl");

        // payload.type 用 \u 转义拼写：解码后仍是 token_count，参考实现照常提取。
        let escaped_payload_type = format!(
            r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"\u0074oken_count","info":{{"total_token_usage":{{"input_tokens":11,"output_tokens":22}}}}}}}}"#,
            corpus_ts(1)
        );
        // 顶层 type 用 \u 转义拼写：解码后仍是 event_msg。
        let escaped_line_type = format!(
            r#"{{"timestamp":"{}","type":"\u0065vent_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":33,"output_tokens":44}}}}}}}}"#,
            corpus_ts(2)
        );
        assert!(
            !escaped_payload_type.contains("\"token_count\""),
            "转义写法不应出现字面量 token_count 判别串"
        );
        assert!(
            !escaped_line_type.contains("\"event_msg\""),
            "转义写法不应出现字面量 event_msg 判别串"
        );

        fs::write(
            &path,
            format!(
                "{escaped_payload_type}\n{escaped_line_type}\n{}\n{}\n",
                token_count_line(&corpus_ts(3), 7),
                format_args!(
                    r#"{{"timestamp":"{}","type":"turn_context","payload":{{}}}}"#,
                    corpus_ts(50)
                ),
            ),
        )
        .unwrap();

        assert_eq!(
            parent_signatures_before(&path, corpus_time(50))
                .expect("父文件已写到 cutoff")
                .len(),
            3,
            "三条 token_count（含两条转义拼写）都应被提取"
        );
        assert_oracle_matches(
            &path,
            &[
                corpus_time(0),
                corpus_time(1),
                corpus_time(2),
                corpus_time(3),
                corpus_time(50),
                corpus_time(51),
            ],
        );
    }

    #[test]
    fn test_parent_signatures_match_reference_on_duplicate_key_spellings() {
        let dir = tempdir().unwrap();

        // 写法一：键与冒号之间有空白（`"timestamp" : "B"`）。后者生效。
        let whitespace = dir.path().join("rollout-dup-whitespace.jsonl");
        fs::write(
            &whitespace,
            format!(
                "{}\n{}\n",
                format_args!(
                    r#"{{"timestamp":"{}","type":"turn_context","timestamp" : "{}","payload":{{}}}}"#,
                    corpus_ts(1),
                    corpus_ts(80)
                ),
                token_count_line(&corpus_ts(3), 7),
            ),
        )
        .unwrap();
        assert!(
            parent_signatures_before(&whitespace, corpus_time(80)).is_ok(),
            "重复键取后者：max_timestamp 应到 ts(80)"
        );
        assert!(parent_signatures_before(&whitespace, corpus_time(81)).is_err());
        assert_oracle_matches(
            &whitespace,
            &[
                corpus_time(1),
                corpus_time(3),
                corpus_time(80),
                corpus_time(81),
            ],
        );

        // 写法二：键名本身被 \u 转义（解码后仍是 timestamp）。后者同样生效。
        let escaped = dir.path().join("rollout-dup-escaped-key.jsonl");
        fs::write(
            &escaped,
            format!(
                "{}\n{}\n",
                format_args!(
                    r#"{{"timestamp":"{}","type":"turn_context","\u0074imestamp":"{}","payload":{{}}}}"#,
                    corpus_ts(2),
                    corpus_ts(81)
                ),
                token_count_line(&corpus_ts(3), 7),
            ),
        )
        .unwrap();
        assert!(
            parent_signatures_before(&escaped, corpus_time(81)).is_ok(),
            "转义键名与字面键名同键，取后者"
        );
        assert!(parent_signatures_before(&escaped, corpus_time(82)).is_err());
        assert_oracle_matches(
            &escaped,
            &[
                corpus_time(2),
                corpus_time(3),
                corpus_time(81),
                corpus_time(82),
            ],
        );
    }

    #[test]
    fn test_parent_signatures_match_reference_on_non_string_fields() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout-non-string-fields.jsonl");
        let contents = [
            // timestamp 非字符串：参考实现 as_str() → None，不贡献 max_timestamp
            r#"{"timestamp":1234,"type":"turn_context","payload":{}}"#.to_string(),
            r#"{"timestamp":{"nested":"2031-01-01T00:00:00Z"},"type":"turn_context","payload":{}}"#
                .to_string(),
            r#"{"timestamp":["2031-01-01T00:00:00Z"],"type":"turn_context","payload":{}}"#
                .to_string(),
            r#"{"timestamp":null,"type":"turn_context","payload":{}}"#.to_string(),
            // type 非字符串 → 不匹配 event_msg（不是解析错误）
            format!(
                r#"{{"timestamp":"{}","type":7,"payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":9}}}}}}}}"#,
                corpus_ts(4)
            ),
            // payload 不是对象：仍须贡献 max_timestamp，不能让整行解析失败
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":"not-an-object"}}"#,
                corpus_ts(5)
            ),
            // payload.type 非字符串
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":9,"info":{{"total_token_usage":{{"input_tokens":9}}}}}}}}"#,
                corpus_ts(6)
            ),
            // 重复 timestamp 键且后者非字符串 → last-wins 覆写成"没有时间戳"
            format!(
                r#"{{"timestamp":"{}","type":"turn_context","timestamp":5,"payload":{{}}}}"#,
                corpus_ts(7)
            ),
            // 顶层不是对象
            "[1,2,3]".to_string(),
            "\"just a string\"".to_string(),
            "42".to_string(),
            // 锚点：唯一有效的 token_count，也提供 max_timestamp
            token_count_line(&corpus_ts(8), 5),
        ]
        .join("\n");
        fs::write(&path, contents + "\n").unwrap();

        assert_eq!(
            parent_signatures_before(&path, corpus_time(8))
                .expect("锚点行提供 max_timestamp")
                .len(),
            1,
            "只有锚点行是有效 token_count"
        );
        assert_oracle_matches(
            &path,
            &[
                corpus_time(3),
                corpus_time(4),
                corpus_time(5),
                corpus_time(6),
                corpus_time(7),
                corpus_time(8),
                corpus_time(9),
            ],
        );
    }

    // ---------------------------------------------------------------------
    // 声明性语义差异（见 `parse_parent_signature_file` 文档注释）：
    // 语法合法、但 `serde_json::Value` 构造不出来，且垃圾落在被跳过子树的行。
    // ---------------------------------------------------------------------

    /// 三类 `Value` 拒绝、跳过式解析接受的垃圾载荷（`"key":value` 片段）
    fn value_hostile_payloads() -> Vec<(&'static str, String)> {
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        vec![
            ("float-overflow", r#""junk":1e400"#.to_string()),
            ("deep-nesting", format!(r#""junk":{deep}"#)),
            ("lone-surrogate", r#""junk":"\uD800""#.to_string()),
        ]
    }

    #[test]
    fn test_parent_signatures_declared_divergence_on_skipped_subtree_junk() {
        let dir = tempdir().unwrap();

        for (name, junk) in value_hostile_payloads() {
            let path = dir
                .path()
                .join(format!("rollout-divergence-max-{name}.jsonl"));
            // 垃圾落在顶层被跳过的 `junk` 键上，该行携带全文件最大 timestamp
            let junk_line = format!(
                r#"{{"timestamp":"{}","type":"turn_context","payload":{{}},{junk}}}"#,
                corpus_ts(9_000)
            );
            assert!(
                serde_json::from_str::<serde_json::Value>(&junk_line).is_err(),
                "{name}: Value 应拒绝该行"
            );
            assert!(
                serde_json::from_str::<IgnoredAny>(&junk_line).is_ok(),
                "{name}: 该行语法应合法"
            );
            fs::write(
                &path,
                format!("{}\n{junk_line}\n", token_count_line(&corpus_ts(1), 1)),
            )
            .unwrap();

            let cutoff = corpus_time(500);
            let reference = parent_signatures_before_reference(&path, cutoff);
            assert!(
                reference
                    .as_ref()
                    .is_err_and(|error| error.contains("尚未写到")),
                "{name}: 参考实现整行丢弃 → 应报尚未写到，实际 {reference:?}"
            );
            let actual = parent_signatures_before(&path, cutoff).unwrap_or_else(|error| {
                panic!("{name}: 新实现应把可解析出的 timestamp 计为写入进度: {error}")
            });
            assert_eq!(actual.len(), 1, "{name}: 签名集合不受影响");

            // 两侧都能作答的 cutoff 上，签名逐字一致
            for cutoff in [corpus_time(0), corpus_time(1)] {
                assert_eq!(
                    parent_signatures_before(&path, cutoff),
                    parent_signatures_before_reference(&path, cutoff),
                    "{name}: cutoff {cutoff} 上签名应一致"
                );
            }
        }
    }

    #[test]
    fn test_parent_signatures_declared_divergence_extends_to_signatures() {
        // 文档注释点名的边界：垃圾落在 token_count 行**被跳过**的兄弟子树时，
        // 差异不止 max_timestamp——参考实现连该行的签名一起丢，新实现照常收下；
        // 缺 timestamp 时新实现还会给出"缺少有效 timestamp"错误。
        let dir = tempdir().unwrap();

        for (name, junk) in value_hostile_payloads() {
            let path = dir
                .path()
                .join(format!("rollout-divergence-sig-{name}.jsonl"));
            let junk_signature_line = format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":77,"output_tokens":88}}}}}},{junk}}}"#,
                corpus_ts(2)
            );
            fs::write(
                &path,
                format!(
                    "{}\n{junk_signature_line}\n{}\n",
                    token_count_line(&corpus_ts(1), 1),
                    format_args!(
                        r#"{{"timestamp":"{}","type":"turn_context","payload":{{}}}}"#,
                        corpus_ts(9_000)
                    ),
                ),
            )
            .unwrap();

            let cutoff = corpus_time(500);
            assert_eq!(
                parent_signatures_before_reference(&path, cutoff)
                    .unwrap()
                    .len(),
                1,
                "{name}: 参考实现丢掉整行（连签名一起）"
            );
            assert_eq!(
                parent_signatures_before(&path, cutoff).unwrap().len(),
                2,
                "{name}: 新实现收下被跳过子树里带垃圾的 token_count 签名"
            );

            let missing = dir
                .path()
                .join(format!("rollout-divergence-missing-{name}.jsonl"));
            let junk_missing_line = format!(
                r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":77}}}}}},{junk}}}"#
            );
            fs::write(
                &missing,
                format!(
                    "{}\n{junk_missing_line}\n",
                    token_count_line(&corpus_ts(1), 1)
                ),
            )
            .unwrap();
            assert!(
                parent_signatures_before_reference(&missing, corpus_time(1)).is_ok(),
                "{name}: 参考实现静默丢行"
            );
            let actual = parent_signatures_before(&missing, corpus_time(1));
            assert!(
                actual
                    .as_ref()
                    .is_err_and(|error| error.contains("缺少有效 timestamp")),
                "{name}: 新实现按缺 timestamp 的 token_count 处理，实际 {actual:?}"
            );
        }
    }

    #[test]
    fn test_parent_signatures_match_reference_on_captured_subtree_junk() {
        // 垃圾落在**被捕获**的子树（info / timestamp 字符串 / 键名）时零差异：
        // Value 捕获或字符串解码失败 → 整行解析失败 → 与参考实现一样跳过。
        let dir = tempdir().unwrap();
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        let path = dir.path().join("rollout-captured-junk.jsonl");
        let contents = [
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":1e400}}}}}}}}"#,
                corpus_ts(9_001)
            ),
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":5}},"aux":{deep}}}}}}}"#,
                corpus_ts(9_002)
            ),
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":5}},"aux":"\uD800"}}}}}}"#,
                corpus_ts(9_003)
            ),
            // 被捕获的 timestamp 字符串里的孤立代理项
            r#"{"timestamp":"\uD800","type":"turn_context","payload":{}}"#.to_string(),
            // 键名里的孤立代理项
            format!(
                r#"{{"timestamp":"{}","\uD800":1,"type":"turn_context","payload":{{}}}}"#,
                corpus_ts(9_004)
            ),
            // 锚点
            token_count_line(&corpus_ts(1), 1),
            format!(
                r#"{{"timestamp":"{}","type":"turn_context","payload":{{}}}}"#,
                corpus_ts(10)
            ),
        ]
        .join("\n");
        fs::write(&path, contents + "\n").unwrap();

        assert!(
            parent_signatures_before(&path, corpus_time(10)).is_ok(),
            "锚点行提供 max_timestamp"
        );
        assert!(
            parent_signatures_before(&path, corpus_time(11)).is_err(),
            "所有 9_00x 行两侧都被跳过，max_timestamp 不应越过 ts(10)"
        );
        assert_oracle_matches(
            &path,
            &[
                corpus_time(0),
                corpus_time(1),
                corpus_time(10),
                corpus_time(11),
                corpus_time(9_005),
            ],
        );
    }

    // ---------------------------------------------------------------------
    // 声明性语义差异之二：serde_json 的私有 `RawValue` 哨兵
    //
    // 依赖图（axum）传递性打开了 serde_json 的 `raw_value` feature，于是
    // `from_str::<Value>` 会把 `{"$serde_json::private::RawValue":"<JSON 文本>"}`
    // **就地展开**成所嵌的值；窄化 visitor 按 JSON 规范读，只把它当未知键。
    // ---------------------------------------------------------------------

    /// 构造 `{"$serde_json::private::RawValue":"<被转义的 JSON 文本>"}` 哨兵对象
    fn raw_value_sentinel(inner_json: &str) -> String {
        format!(
            r#"{{"$serde_json::private::RawValue":{}}}"#,
            serde_json::Value::String(inner_json.to_string())
        )
    }

    #[test]
    fn test_parent_signatures_declared_divergence_on_raw_value_sentinel() {
        let dir = tempdir().unwrap();

        // 1) 哨兵落在 payload 位置：Value 展开出真正的 token_count payload，
        //    本实现只看到一个未知键 → 该行不贡献签名。
        let payload_path = dir.path().join("rollout-sentinel-payload.jsonl");
        let sentinel_payload_line = format!(
            r#"{{"timestamp":"{}","type":"event_msg","payload":{}}}"#,
            corpus_ts(1),
            raw_value_sentinel(
                r#"{"type":"token_count","info":{"total_token_usage":{"input_tokens":42,"output_tokens":7}}}"#
            )
        );
        assert!(
            !sentinel_payload_line.contains(r#""type":"token_count""#),
            "哨兵内层是被转义的 JSON 文本，不应出现字面量判别串"
        );
        fs::write(&payload_path, format!("{sentinel_payload_line}\n")).unwrap();

        let cutoff = corpus_time(1);
        let reference = parent_signatures_before_reference(&payload_path, cutoff)
            .expect("顶层 timestamp 让参考实现认为已写到 cutoff");
        assert_eq!(reference.len(), 1, "参考实现展开哨兵后提取到签名");
        assert_eq!(
            reference[0].total.as_ref().unwrap().input,
            Some(42),
            "参考实现读到的是哨兵内层的计数器"
        );
        let actual = parent_signatures_before(&payload_path, cutoff)
            .expect("顶层 timestamp 本实现照样读得到");
        assert!(
            actual.is_empty(),
            "本实现按普通 JSON 读：哨兵只是未知键，该行无签名，实际 {actual:?}"
        );

        // 2) 哨兵落在顶层：Value 展开出一整行 token_count 事件（连 timestamp 一起），
        //    本实现读不到任何键 → 该行既不贡献签名也不贡献 max_timestamp。
        let top_path = dir.path().join("rollout-sentinel-top.jsonl");
        let sentinel_top_line = raw_value_sentinel(&token_count_line(&corpus_ts(2), 9));
        fs::write(
            &top_path,
            format!(
                "{}\n{sentinel_top_line}\n",
                token_count_line(&corpus_ts(1), 1)
            ),
        )
        .unwrap();

        // cutoff 落在锚点行上：两侧都只看得到锚点签名 → 零差异
        assert_oracle_matches(&top_path, &[corpus_time(0), corpus_time(1)]);
        // cutoff 落在哨兵行时刻：参考实现认为文件已写到 ts(2) 并给出两条签名，
        // 本实现读不到那个 timestamp → "尚未写到"
        let reference_top = parent_signatures_before_reference(&top_path, corpus_time(2))
            .expect("参考实现展开哨兵后 max_timestamp 到 ts(2)");
        assert_eq!(reference_top.len(), 2, "参考实现连哨兵行的签名一起收下");
        let actual_top = parent_signatures_before(&top_path, corpus_time(2));
        assert!(
            actual_top
                .as_ref()
                .is_err_and(|error| error.contains("尚未写到")),
            "本实现读不到哨兵里的 timestamp，实际 {actual_top:?}"
        );

        // 3) 哨兵落在被捕获的 timestamp 字符串位置：Value 展开成 String → 时间戳成立，
        //    本实现的 MaybeStr 看到的是对象 → 等价于"没有字符串"。
        let ts_path = dir.path().join("rollout-sentinel-timestamp.jsonl");
        let sentinel_ts_line = format!(
            r#"{{"timestamp":{},"type":"turn_context","payload":{{}}}}"#,
            raw_value_sentinel(&format!(r#""{}""#, corpus_ts(9_000)))
        );
        fs::write(
            &ts_path,
            format!(
                "{}\n{sentinel_ts_line}\n",
                token_count_line(&corpus_ts(1), 1)
            ),
        )
        .unwrap();
        assert!(
            parent_signatures_before_reference(&ts_path, corpus_time(9_000)).is_ok(),
            "参考实现展开哨兵后拿到 ts(9_000)"
        );
        let actual_ts = parent_signatures_before(&ts_path, corpus_time(9_000));
        assert!(
            actual_ts
                .as_ref()
                .is_err_and(|error| error.contains("尚未写到")),
            "本实现视为 timestamp 非字符串，实际 {actual_ts:?}"
        );
    }

    #[test]
    fn test_parent_signatures_declared_divergence_on_nested_skipped_key_junk() {
        // 被跳过子树里的**键名**与值一样宽松：孤立代理项落在嵌套键名上时，
        // Value 拒绝整行、`IgnoredAny` 照常跳过 → 属声明性差异类。
        let dir = tempdir().unwrap();

        for (name, junk_line) in [
            (
                "skipped-top-key",
                format!(
                    r#"{{"timestamp":"{}","type":"turn_context","payload":{{}},"junk":{{"\uD800":0}}}}"#,
                    corpus_ts(9_000)
                ),
            ),
            (
                "skipped-payload-key",
                format!(
                    r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"agent_message","extra":{{"\uD800":0}}}}}}"#,
                    corpus_ts(9_000)
                ),
            ),
        ] {
            let path = dir
                .path()
                .join(format!("rollout-nested-surrogate-{name}.jsonl"));
            assert!(
                serde_json::from_str::<serde_json::Value>(&junk_line).is_err(),
                "{name}: Value 应拒绝该行"
            );
            assert!(
                serde_json::from_str::<IgnoredAny>(&junk_line).is_ok(),
                "{name}: 该行语法应合法"
            );
            fs::write(
                &path,
                format!("{}\n{junk_line}\n", token_count_line(&corpus_ts(1), 1)),
            )
            .unwrap();

            let cutoff = corpus_time(500);
            let reference = parent_signatures_before_reference(&path, cutoff);
            assert!(
                reference
                    .as_ref()
                    .is_err_and(|error| error.contains("尚未写到")),
                "{name}: 参考实现整行丢弃 → 应报尚未写到，实际 {reference:?}"
            );
            let actual = parent_signatures_before(&path, cutoff).unwrap_or_else(|error| {
                panic!("{name}: 本实现应把可解析出的 timestamp 计为写入进度: {error}")
            });
            assert_eq!(actual.len(), 1, "{name}: 签名集合不受影响");
        }

        // 对照：孤立代理项落在**解析路径上**的键名（顶层键 / payload 键）时，
        // 两个实现一起拒绝整行，零差异。
        let path = dir.path().join("rollout-path-surrogate-key.jsonl");
        let contents = [
            format!(
                r#"{{"timestamp":"{}","\uD800":1,"type":"turn_context","payload":{{}}}}"#,
                corpus_ts(9_001)
            ),
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"\uD800":1,"type":"token_count","info":{{"total_token_usage":{{"input_tokens":5}}}}}}}}"#,
                corpus_ts(9_002)
            ),
            token_count_line(&corpus_ts(1), 1),
            format!(
                r#"{{"timestamp":"{}","type":"turn_context","payload":{{}}}}"#,
                corpus_ts(10)
            ),
        ]
        .join("\n");
        fs::write(&path, contents + "\n").unwrap();
        assert!(
            parent_signatures_before(&path, corpus_time(10)).is_ok(),
            "锚点行提供 max_timestamp"
        );
        assert!(
            parent_signatures_before(&path, corpus_time(11)).is_err(),
            "解析路径上的孤立代理项让两侧一起丢行，max_timestamp 不应越过 ts(10)"
        );
        assert_oracle_matches(
            &path,
            &[
                corpus_time(0),
                corpus_time(1),
                corpus_time(10),
                corpus_time(11),
                corpus_time(9_003),
            ],
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_parent_signatures_open_error_takes_precedence_over_cached_content_error() {
        use std::os::unix::fs::PermissionsExt;

        // open 先行：即使新鲜度戳完全不变，`open` 失败也必须给出当次的新鲜错误，
        // 而不是拿旧快照里的内容型错误顶包。
        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout-chmod-open-error.jsonl");
        fs::write(&path, format!("{}\n", token_count_line(&corpus_ts(1), 1))).unwrap();

        // 先用一次内容型错误把缓存填上
        let cutoff = corpus_time(9_000);
        let cached = parent_signatures_before(&path, cutoff);
        assert!(
            cached
                .as_ref()
                .is_err_and(|error| error.contains("尚未写到")),
            "缓存里应存着内容型错误，实际 {cached:?}"
        );

        // chmod 000 只改 ctime/mode：(mtime_ns, size, dev, ino) 全不变，
        // 因此"先查缓存"的实现会继续吐旧的内容型错误。
        let original = fs::metadata(&path).unwrap().permissions();
        let before = fs::metadata(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let after = fs::metadata(&path).unwrap();
        assert_eq!(
            (metadata_modified_nanos(&before), before.len()),
            (metadata_modified_nanos(&after), after.len()),
            "chmod 不应改动 mtime/size"
        );

        // root 无视 mode：先探一次，打不开才有断言意义
        let readable_anyway = fs::File::open(&path).is_ok();
        let outcome = (!readable_anyway).then(|| {
            (
                parent_signatures_before(&path, cutoff),
                parent_signatures_before_reference(&path, cutoff),
            )
        });
        // 恢复权限，否则 tempdir 清理可能受影响
        fs::set_permissions(&path, original).unwrap();

        let Some((actual, expected)) = outcome else {
            eprintln!("[skip] 当前用户可无视 mode 0o000（疑似 root），跳过断言");
            return;
        };
        assert!(
            expected
                .as_ref()
                .is_err_and(|error| error.starts_with("无法打开父 rollout")),
            "参考实现应报打开失败，实际 {expected:?}"
        );
        assert_eq!(
            actual, expected,
            "open 错误必须是当次新鲜的，且与参考实现逐字一致"
        );
    }

    #[test]
    fn test_parent_signatures_match_reference_on_duplicate_captured_keys() {
        // 重复的 payload / info / 判别串一律"后者整体替换前者"（不是合并），
        // 与本 crate 开启 `preserve_order` 的 Value 一致。
        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout-duplicate-captured.jsonl");
        let info_a = r#"{"total_token_usage":{"input_tokens":11,"output_tokens":1}}"#;
        let info_b = r#"{"total_token_usage":{"input_tokens":22,"output_tokens":2}}"#;

        let contents = [
            // 1. 重复 payload（对象→对象）：后者**替换**而非合并。后者没有 type，
            //    整行因此不再是 token_count；若是合并就会错误地留下前者的 type。
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{info_a}}},"payload":{{"info":{info_b}}}}}"#,
                corpus_ts(2)
            ),
            // 2. 重复 payload：前者缺 type、后者完整 → 后者生效，产出签名
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"info":{info_a}}},"payload":{{"type":"token_count","info":{info_b}}}}}"#,
                corpus_ts(3)
            ),
            // 3. 重复 info：后者生效
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{info_a},"info":{info_b}}}}}"#,
                corpus_ts(4)
            ),
            // 4. 判别串「字符串→非字符串」：后者把它覆写成"没有字符串" → 不匹配
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","type":7,"payload":{{"type":"token_count","info":{info_b}}}}}"#,
                corpus_ts(5)
            ),
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","type":9,"info":{info_b}}}}}"#,
                corpus_ts(6)
            ),
            // 5. 判别串「非字符串→字符串」：后者生效 → 匹配
            format!(
                r#"{{"timestamp":"{}","type":7,"type":"event_msg","payload":{{"type":9,"type":"token_count","info":{info_b}}}}}"#,
                corpus_ts(7)
            ),
        ]
        .join("\n");
        fs::write(&path, contents + "\n").unwrap();

        let signatures =
            parent_signatures_before(&path, corpus_time(7)).expect("末行提供 max_timestamp");
        assert_eq!(
            signatures.len(),
            3,
            "只有第 2/3/6 行是有效 token_count（第 1 行证明 payload 是替换而非合并）"
        );
        assert!(
            signatures
                .iter()
                .all(|signature| signature.total.as_ref().unwrap().input == Some(22)),
            "重复键一律取后者，实际 {signatures:?}"
        );
        assert_oracle_matches(
            &path,
            &[
                corpus_time(1),
                corpus_time(2),
                corpus_time(3),
                corpus_time(4),
                corpus_time(5),
                corpus_time(6),
                corpus_time(7),
                corpus_time(8),
            ],
        );
    }

    #[test]
    fn test_parent_signatures_match_reference_on_overflow_in_captured_positions() {
        // `1e999` 落在**被捕获**的位置（timestamp / type / payload.type / info）时，
        // 两个实现都必须真正读出这个值 → 一起整行失败，零差异。
        // （对比：落在被跳过子树里才是声明性差异类。）
        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout-overflow-captured.jsonl");
        let contents = [
            r#"{"timestamp":1e999,"type":"turn_context","payload":{}}"#.to_string(),
            format!(
                r#"{{"timestamp":"{}","type":1e999,"payload":{{}}}}"#,
                corpus_ts(9_001)
            ),
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":1e999,"info":{{"total_token_usage":{{"input_tokens":5}}}}}}}}"#,
                corpus_ts(9_002)
            ),
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":1e999}}}}"#,
                corpus_ts(9_003)
            ),
            // 锚点
            token_count_line(&corpus_ts(1), 1),
            format!(
                r#"{{"timestamp":"{}","type":"turn_context","payload":{{}}}}"#,
                corpus_ts(10)
            ),
        ]
        .join("\n");
        fs::write(&path, contents + "\n").unwrap();

        assert_eq!(
            parent_signatures_before(&path, corpus_time(10))
                .expect("锚点行提供 max_timestamp")
                .len(),
            1,
            "只有锚点行是有效 token_count"
        );
        assert!(
            parent_signatures_before(&path, corpus_time(11)).is_err(),
            "所有 9_00x 行两侧都被跳过，max_timestamp 不应越过 ts(10)"
        );
        assert_oracle_matches(
            &path,
            &[
                corpus_time(0),
                corpus_time(1),
                corpus_time(10),
                corpus_time(11),
                corpus_time(9_004),
            ],
        );
    }

    /// 把文件 mtime 设成固定值，用于构造"同 (mtime, size) 但不同 inode"的替换
    #[cfg(unix)]
    fn set_fixed_mtime(path: &Path, time: SystemTime) {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(time)
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn test_parent_signatures_stamp_detects_rename_replacement() {
        // 新鲜度戳在 Unix 上带 (dev, ino)：把另一个同尺寸、同 mtime 的文件 rename
        // 覆盖上来时，只有 inode 会变，缓存必须失效。
        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout-stamp.jsonl");
        let stamp_line = |input: u64| {
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"output_tokens":7}}}}}}}}"#,
                corpus_ts(1)
            )
        };
        let original = format!("{}\n", stamp_line(100_000));
        let replacement_contents = format!("{}\n", stamp_line(200_000));
        assert_eq!(
            original.len(),
            replacement_contents.len(),
            "两份内容必须同尺寸，才能让 (mtime, size) 完全相同"
        );

        let fixed = SystemTime::UNIX_EPOCH + std::time::Duration::new(1_800_000_000, 123_456_789);
        fs::write(&path, &original).unwrap();
        set_fixed_mtime(&path, fixed);
        let before = fs::metadata(&path).unwrap();

        let first = parent_signatures_before(&path, corpus_time(1)).unwrap();
        assert_eq!(
            first,
            parent_signatures_before_reference(&path, corpus_time(1)).unwrap()
        );
        assert_eq!(
            first[0].total.as_ref().unwrap().input,
            Some(100_000),
            "首次查询应读到原始内容"
        );

        // 先在别处建好替换文件（与原文件同时存在 → inode 必不同），再 rename 覆盖
        let staged = dir.path().join("rollout-stamp.replacement");
        fs::write(&staged, &replacement_contents).unwrap();
        set_fixed_mtime(&staged, fixed);
        fs::rename(&staged, &path).unwrap();

        let after = fs::metadata(&path).unwrap();
        assert_eq!(before.len(), after.len(), "size 必须相同");
        assert_eq!(
            metadata_modified_nanos(&before),
            metadata_modified_nanos(&after),
            "mtime 必须相同：本用例只考验 (dev, ino)"
        );
        {
            use std::os::unix::fs::MetadataExt;
            assert_ne!(before.ino(), after.ino(), "rename 替换后 inode 必须不同");
        }

        let second = parent_signatures_before(&path, corpus_time(1)).unwrap();
        assert_eq!(
            second,
            parent_signatures_before_reference(&path, corpus_time(1)).unwrap()
        );
        assert_eq!(
            second[0].total.as_ref().unwrap().input,
            Some(200_000),
            "同 (mtime, size) 的 rename 替换必须触发重解析"
        );
    }

    #[test]
    fn test_parent_signatures_cutoff_is_nanosecond_exact() {
        // 钉住相对 HEAD 的刻意改进：HEAD 的 (path, cutoff_micros) 缓存把 cutoff
        // 截断到微秒，下面两个相距 400ns 的 cutoff 会被它折叠成同一个答案。
        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout-nanos.jsonl");
        fs::write(
            &path,
            format!(
                "{}\n{}\n",
                token_count_line("2026-07-10T03:00:01.000000500Z", 1),
                format_args!(
                    r#"{{"timestamp":"{}","type":"turn_context","payload":{{}}}}"#,
                    corpus_ts(90)
                ),
            ),
        )
        .unwrap();

        let parse = |raw: &str| {
            DateTime::parse_from_rfc3339(raw)
                .unwrap()
                .with_timezone(&Utc)
        };
        let before = parse("2026-07-10T03:00:01.000000300Z");
        let after = parse("2026-07-10T03:00:01.000000700Z");
        assert_eq!((after - before).num_nanoseconds(), Some(400));

        assert!(
            parent_signatures_before(&path, before).unwrap().is_empty(),
            "300ns 的 cutoff 早于 500ns 的签名"
        );
        assert_eq!(
            parent_signatures_before(&path, after).unwrap().len(),
            1,
            "700ns 的 cutoff 晚于 500ns 的签名"
        );
        assert_oracle_matches(&path, &[before, after]);
    }

    /// 性能基准：`cargo test -- --ignored --nocapture bench_parent_signatures`
    #[test]
    #[ignore = "性能基准，需显式 --ignored 运行"]
    fn bench_parent_signatures_vs_reference() {
        const LINES: usize = 50_000;
        const CHILDREN: usize = 20;

        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout-bench.jsonl");
        let (contents, candidates) =
            synthetic_rollout_corpus(42, LINES, 600, CorpusProfile::Realistic);
        fs::write(&path, &contents).unwrap();

        let total = contents.lines().count();
        let step = candidates.len() / CHILDREN.max(1);
        let cutoffs: Vec<DateTime<Utc>> = (0..CHILDREN)
            .map(|index| candidates[(index * step).min(candidates.len() - 1)])
            .collect();

        clear_codex_replay_caches();
        let started = Instant::now();
        for cutoff in &cutoffs {
            let _ = parent_signatures_before(&path, *cutoff);
        }
        let new_elapsed = started.elapsed();

        let started = Instant::now();
        for cutoff in &cutoffs {
            let _ = parent_signatures_before_reference(&path, *cutoff);
        }
        let reference_elapsed = started.elapsed();

        println!(
            "[bench] 语料 {} 行 / {:.1} MiB，token_count 行 {}",
            total,
            contents.len() as f64 / (1024.0 * 1024.0),
            candidates.len(),
        );
        println!(
            "[bench] {CHILDREN} 个子 cutoff：新实现 {new_elapsed:?}，参考实现 {reference_elapsed:?}，加速 {:.1}x",
            reference_elapsed.as_secs_f64() / new_elapsed.as_secs_f64().max(f64::MIN_POSITIVE)
        );

        // 冷缓存单次解析成本（窄化解析器的单遍成本，不含缓存收益）
        clear_codex_replay_caches();
        let started = Instant::now();
        let _ = parent_signatures_before(&path, cutoffs[CHILDREN / 2]);
        let cold = started.elapsed();
        let started = Instant::now();
        let _ = parent_signatures_before_reference(&path, cutoffs[CHILDREN / 2]);
        let reference_single = started.elapsed();
        println!(
            "[bench] 单次冷解析：新实现 {cold:?}，参考实现 {reference_single:?}，加速 {:.1}x",
            reference_single.as_secs_f64() / cold.as_secs_f64().max(f64::MIN_POSITIVE)
        );
    }
}
