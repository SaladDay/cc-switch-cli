//! Claude Code 会话日志使用追踪
//!
//! 从 ~/.claude/projects/ 下的 JSONL 会话文件中提取 token 使用数据，
//! 实现无代理模式下的使用统计。
//!
//! ## 数据流
//! ```text
//! ~/.claude/projects/*/*.jsonl → 增量解析 → 去重 → 费用计算 → proxy_request_logs 表
//! ```

use crate::config::get_claude_config_dir;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::proxy::usage::calculator::{CostCalculator, ModelPricing};
use crate::proxy::usage::parser::TokenUsage;
use crate::services::session_usage_driver::{
    resume_hint_from_shared_cursor, save_resume_hint, scan_jsonl_incremental,
    shared_tail_fingerprint_from_file, unchanged_jsonl_identity_is_suspicious,
};
use crate::services::usage_stats::{
    effective_usage_log_filter, find_model_pricing, should_skip_session_insert, DedupKey,
};
use crate::session_manager::scan_cache_store::{ScanCacheStore, SyncResumeHint};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

const SESSION_SYNC_INTERVAL_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, Default)]
struct ClaudeSyncState {
    last_modified: i64,
    last_line_offset: i64,
    last_byte_offset: Option<i64>,
    last_tail_fingerprint: Option<i64>,
}

/// 同步结果
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSyncResult {
    pub imported: u32,
    pub skipped: u32,
    pub files_scanned: u32,
    pub suspected_duplicates: u32,
    pub deferred_files: u32,
    pub errors: Vec<String>,
}

/// 数据来源分布
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSourceSummary {
    pub data_source: String,
    pub request_count: u32,
    pub total_cost_usd: String,
}

impl SessionSyncResult {
    pub fn merge(&mut self, other: SessionSyncResult) {
        self.imported = self.imported.saturating_add(other.imported);
        self.skipped = self.skipped.saturating_add(other.skipped);
        self.files_scanned = self.files_scanned.saturating_add(other.files_scanned);
        self.suspected_duplicates = self
            .suspected_duplicates
            .saturating_add(other.suspected_duplicates);
        self.deferred_files = self.deferred_files.saturating_add(other.deferred_files);
        self.errors.extend(other.errors);
    }
}

/// Serializes session usage imports within this process. Callers that need a
/// multi-step operation (backup -> reset -> reimport) hold the same lock for
/// the complete sequence.
pub(crate) fn session_sync_mutex() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Serializes session imports across both threads and cc-switch processes.
///
/// The file lock lives beside the database so the TUI, daemon, and one-shot
/// CLI commands all rendezvous on the same inode. In-memory test databases do
/// not have a cross-process representation and therefore use only the process
/// mutex.
pub(crate) struct SessionSyncGuard {
    file: Option<File>,
    _process: MutexGuard<'static, ()>,
}

// Rust 1.91's std file-lock implementation returns Unsupported on Android
// without calling Bionic's available flock(2). Keep the workaround scoped to
// Android so every other target retains the existing std behavior.
#[cfg(target_os = "android")]
fn android_session_sync_flock(file: &File, operation: libc::c_int) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;

    if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn lock_session_sync_file(file: &File) -> std::io::Result<()> {
    #[cfg(target_os = "android")]
    return android_session_sync_flock(file, libc::LOCK_EX);

    #[cfg(not(target_os = "android"))]
    file.lock()
}

fn unlock_session_sync_file(file: &File) -> std::io::Result<()> {
    #[cfg(target_os = "android")]
    return android_session_sync_flock(file, libc::LOCK_UN);

    #[cfg(not(target_os = "android"))]
    file.unlock()
}

impl Drop for SessionSyncGuard {
    fn drop(&mut self) {
        if let Some(file) = &self.file {
            let _ = unlock_session_sync_file(file);
        }
    }
}

fn open_session_sync_lock(path: &Path) -> Result<File, AppError> {
    #[cfg(not(unix))]
    if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(AppError::InvalidInput(format!(
            "会话用量同步锁不能是符号链接: {}",
            path.display()
        )));
    }

    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;

        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| AppError::io(path, error))?
    };

    #[cfg(not(unix))]
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| AppError::io(path, error))?;

    let metadata = file.metadata().map_err(|error| AppError::io(path, error))?;
    if !metadata.is_file() {
        return Err(AppError::InvalidInput(format!(
            "会话用量同步锁不是普通文件: {}",
            path.display()
        )));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;

        if metadata.nlink() != 1 {
            return Err(AppError::InvalidInput(format!(
                "会话用量同步锁不能是硬链接: {}",
                path.display()
            )));
        }

        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| AppError::io(path, error))?;
    }

    Ok(file)
}

/// Acquire the process-wide mutex and then the database-scoped file lock. A
/// panic in an earlier sync must not permanently disable future imports; the
/// protected state is committed through SQLite transactions, so recovering a
/// poisoned mutex is safe.
pub(crate) fn acquire_session_sync_guard(db: &Database) -> Result<SessionSyncGuard, AppError> {
    let process = match session_sync_mutex().lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            log::warn!("Session usage sync lock was poisoned; recovering it");
            poisoned.into_inner()
        }
    };

    let file = db
        .session_usage_lock_path()
        .map(|path| {
            let file = open_session_sync_lock(&path)?;
            lock_session_sync_file(&file).map_err(|error| AppError::io(&path, error))?;
            Ok::<_, AppError>(file)
        })
        .transpose()?;

    Ok(SessionSyncGuard {
        file,
        _process: process,
    })
}

/// 从 JSONL 中解析出的 assistant 消息使用数据
#[derive(Debug)]
struct ParsedAssistantUsage {
    message_id: String,
    model: String,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_creation_tokens: u32,
    stop_reason: Option<String>,
    timestamp: Option<String>,
    session_id: Option<String>,
}

/// 窄结构体：仅反序列化 usage 追踪所需字段，避免为每行构建完整
/// `serde_json::Value`（尤其是多兆字节的 tool_result 行）。所有字段容忍缺失，
/// 语义与旧逐字段 `.get()` 读取保持一致。
#[derive(Debug, Deserialize)]
struct NarrowClaudeLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    message: Option<NarrowClaudeMessage>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NarrowClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<NarrowClaudeUsage>,
    stop_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct NarrowClaudeUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
}

/// 单文件批量提交的分段大小：超大文件每累计 N 行 INSERT 提交一次，
/// 限制单个事务的 WAL 增长与内存占用。
pub(crate) const SESSION_LOG_COMMIT_BATCH: u32 = 500;

/// 每个同步周期内的模型定价缓存：按 model 名缓存 `model_pricing` 查询结果，
/// 避免对每条消息重复查库。
pub(crate) type PricingCache = HashMap<String, Option<ModelPricing>>;

/// 从缓存获取模型定价；未命中则查库并写回缓存。
pub(crate) fn cached_model_pricing(
    conn: &rusqlite::Connection,
    cache: &mut PricingCache,
    model: &str,
) -> Option<ModelPricing> {
    if let Some(hit) = cache.get(model) {
        return hit.clone();
    }
    let pricing = find_model_pricing(conn, model);
    cache.insert(model.to_string(), pricing.clone());
    pricing
}

/// 使用统计同步的进程内进度：TUI 在同步进行时读取它显示 "x/y 文件" 并周期
/// 刷新数字（CLI 构建里 `notify_log_recorded` 是空实现，没有别的进度通道）。
/// 用全局原子量而非回调层层传递，保持各 sync 函数签名稳定。
pub mod sync_progress {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    static ACTIVE: AtomicBool = AtomicBool::new(false);
    static FILES_DONE: AtomicU32 = AtomicU32::new(0);
    static FILES_TOTAL: AtomicU32 = AtomicU32::new(0);

    /// 同步周期存续期间持有；Drop 时无论成败都清除 active 标志。
    pub(crate) struct SyncProgressGuard;

    impl Drop for SyncProgressGuard {
        fn drop(&mut self) {
            ACTIVE.store(false, Ordering::Relaxed);
        }
    }

    pub(crate) fn begin() -> SyncProgressGuard {
        FILES_DONE.store(0, Ordering::Relaxed);
        FILES_TOTAL.store(0, Ordering::Relaxed);
        ACTIVE.store(true, Ordering::Relaxed);
        SyncProgressGuard
    }

    pub(crate) fn add_total(n: u32) {
        FILES_TOTAL.fetch_add(n, Ordering::Relaxed);
    }

    pub(crate) fn add_done(n: u32) {
        FILES_DONE.fetch_add(n, Ordering::Relaxed);
    }

    /// 同步进行中返回 `(已处理, 总数)`，空闲返回 None。
    pub fn snapshot() -> Option<(u32, u32)> {
        if !ACTIVE.load(Ordering::Relaxed) {
            return None;
        }
        Some((
            FILES_DONE.load(Ordering::Relaxed),
            FILES_TOTAL.load(Ordering::Relaxed),
        ))
    }
}

pub fn sync_all_session_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let _sync_guard = acquire_session_sync_guard(db)?;
    sync_all_session_usage_unlocked(db)
}

/// Rebuild Codex session usage from its local rollout logs.
///
/// The shared sync guard intentionally covers the whole backup -> reset ->
/// reimport sequence. A failed backup returns before any usage row is
/// removed. Once reset succeeds, the Usage view must be notified even when
/// the reimport is empty or fails so it never keeps rendering stale rows.
pub(crate) fn rebuild_codex_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let _sync_guard = acquire_session_sync_guard(db)?;
    db.backup_database_file()?;
    db.reset_codex_usage()?;

    let result = {
        let _progress = sync_progress::begin();
        let _durability = db.bulk_import_durability_guard();
        crate::services::session_usage_codex::sync_codex_usage(db)
    };
    crate::usage_events::notify_log_recorded();
    result
}

/// Synchronization core for callers that already hold [`session_sync_mutex`].
pub(crate) fn sync_all_session_usage_unlocked(
    db: &Database,
) -> Result<SessionSyncResult, AppError> {
    let _progress = sync_progress::begin();
    // 导入周期内本连接临时 synchronous=NORMAL（守卫恢复 FULL）：批量事务
    // 不逐次 fsync，HDD/macOS 上是首次导入的主要开销；usage 行可从源文件
    // 重建，主库其余权威配置的常规写入路径不受影响。
    let _durability = db.bulk_import_durability_guard();
    let mut result = SessionSyncResult {
        imported: 0,
        skipped: 0,
        files_scanned: 0,
        suspected_duplicates: 0,
        deferred_files: 0,
        errors: vec![],
    };
    merge_sync_step(&mut result, "Claude", sync_claude_session_logs(db));
    merge_sync_step(
        &mut result,
        "Codex",
        crate::services::session_usage_codex::sync_codex_usage(db),
    );
    merge_sync_step(
        &mut result,
        "Gemini",
        crate::services::session_usage_gemini::sync_gemini_usage(db),
    );
    merge_sync_step(
        &mut result,
        "OpenCode",
        crate::services::session_usage_opencode::sync_opencode_usage(db),
    );
    merge_sync_step(
        &mut result,
        "Pi",
        crate::services::session_usage_pi::sync_pi_usage(db),
    );
    if result.imported > 0 {
        crate::usage_events::notify_log_recorded();
    }
    Ok(result)
}

fn merge_sync_step(
    result: &mut SessionSyncResult,
    name: &str,
    step: Result<SessionSyncResult, AppError>,
) {
    match step {
        Ok(step_result) => result.merge(step_result),
        Err(error) => result.errors.push(format!("{name}: {error}")),
    }
}

pub(crate) fn run_session_usage_sync_cycle_best_effort(db: &Database, context: &str) {
    match run_session_usage_sync_cycle(db, context) {
        Ok(_) => {}
        Err(error) => log::warn!("Session usage sync failed ({context}): {error}"),
    }
}

pub(crate) fn run_session_usage_sync_cycle(
    db: &Database,
    context: &str,
) -> Result<SessionSyncResult, AppError> {
    let mut result = SessionSyncResult {
        imported: 0,
        skipped: 0,
        files_scanned: 0,
        suspected_duplicates: 0,
        deferred_files: 0,
        errors: vec![],
    };

    match db.backfill_missing_usage_costs() {
        Ok(updated) if updated > 0 => {
            log::info!("Usage cost backfill completed ({context}): updated={updated}");
        }
        Ok(_) => log::debug!("No missing usage costs to backfill ({context})"),
        Err(error) => {
            let message = format!("Usage cost backfill failed: {error}");
            log::warn!("{message} ({context})");
            result.errors.push(message);
        }
    }

    let sync_result = sync_all_session_usage(db)?;
    result.merge(sync_result);
    log_session_usage_sync_result(&result, context);
    Ok(result)
}

fn log_session_usage_sync_result(result: &SessionSyncResult, context: &str) {
    if result.imported > 0 || !result.errors.is_empty() {
        log::info!(
            "Session usage sync completed ({context}): imported={}, skipped={}, files={}, errors={}",
            result.imported,
            result.skipped,
            result.files_scanned,
            result.errors.len()
        );
        for error in result.errors.iter().take(3) {
            log::warn!("Session usage sync error ({context}): {error}");
        }
    } else {
        log::debug!("No new session usage logs to sync ({context})");
    }
}

pub(crate) fn spawn_periodic_session_usage_sync(
    db: Arc<Database>,
    context: &'static str,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run_session_usage_sync_cycle_on_blocking_thread(db.clone(), format!("{context}-initial"))
            .await;

        let mut interval = tokio::time::interval(Duration::from_secs(SESSION_SYNC_INTERVAL_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            run_periodic_session_usage_sync_tick_on_blocking_thread(
                db.clone(),
                format!("{context}-periodic"),
            )
            .await;
        }
    })
}

async fn run_session_usage_sync_cycle_on_blocking_thread(db: Arc<Database>, context: String) {
    let task_context = context.clone();
    match tokio::task::spawn_blocking(move || {
        // 周期同步运行在与 daemon/proxy 共享 `Arc<Database>` 的进程里。导入
        // 优先重开指向同一文件的独立连接：耐久性守卫只降级导入连接（共享
        // 连接上的 proxy 日志、failover 等权威写入保持 FULL），批量事务也
        // 不会经由进程内 mutex 阻塞共享连接的读写。重开失败（内存库测试
        // 环境等）回退共享连接。
        match db.reopen_for_import() {
            Ok(import_db) => run_session_usage_sync_cycle_best_effort(&import_db, &task_context),
            Err(error) => {
                log::debug!("独立导入连接打开失败，回退共享连接 ({task_context}): {error}");
                run_session_usage_sync_cycle_best_effort(&db, &task_context);
            }
        }
    })
    .await
    {
        Ok(()) => {}
        Err(error) => log::warn!("Session usage sync task failed ({context}): {error}"),
    }
}

async fn run_periodic_session_usage_sync_tick_on_blocking_thread(
    db: Arc<Database>,
    context: String,
) {
    run_session_usage_sync_cycle_on_blocking_thread(db, context).await;
}

/// Load the shared Claude cursor columns added by upstream schema v18.
/// A failed read aborts the Claude pass so missing cursor state cannot turn
/// into a full historical replay after detail rows have been rolled up.
fn load_claude_sync_states(db: &Database) -> Result<HashMap<String, ClaudeSyncState>, AppError> {
    let conn = lock_conn!(db.conn);
    let mut stmt = conn
        .prepare(
            "SELECT file_path, last_modified, last_line_offset, last_byte_offset,
                    last_tail_fingerprint
             FROM session_log_sync",
        )
        .map_err(|e| AppError::Database(format!("预取 Claude 同步游标失败: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                ClaudeSyncState {
                    last_modified: row.get(1)?,
                    last_line_offset: row.get(2)?,
                    last_byte_offset: row.get(3)?,
                    last_tail_fingerprint: row.get(4)?,
                },
            ))
        })
        .map_err(|e| AppError::Database(format!("预取 Claude 同步游标失败: {e}")))?;
    rows.collect::<Result<HashMap<_, _>, _>>()
        .map_err(|e| AppError::Database(format!("预取 Claude 同步游标失败: {e}")))
}

/// 同步 Claude Code 会话日志到使用统计数据库
pub fn sync_claude_session_logs(db: &Database) -> Result<SessionSyncResult, AppError> {
    let projects_dir = get_claude_config_dir().join("projects");
    if !projects_dir.exists() {
        return Ok(SessionSyncResult {
            imported: 0,
            skipped: 0,
            files_scanned: 0,
            suspected_duplicates: 0,
            deferred_files: 0,
            errors: vec![],
        });
    }

    let mut result = SessionSyncResult {
        imported: 0,
        skipped: 0,
        files_scanned: 0,
        suspected_duplicates: 0,
        deferred_files: 0,
        errors: vec![],
    };

    // 收集所有 .jsonl 文件（已按 mtime 降序，最近的历史最先入库）
    let jsonl_files = collect_jsonl_files(&projects_dir);

    // 一次性读取全部同步状态，避免对每个文件单独查询数据库。
    let sync_states = load_claude_sync_states(db)?;

    // 本次同步周期共享的定价缓存，避免每条消息重复查 model_pricing 表。
    let mut pricing_cache = PricingCache::new();

    // sidecar 字节续传提示：打不开时优雅降级为行 offset 路径。
    let resume_store = ScanCacheStore::open()
        .inspect_err(|e| log::debug!("[SESSION-SYNC] sidecar 打开失败，禁用字节续传: {e}"))
        .ok();

    // fix 2：一次性预载全部续传提示（一次全表查询，类比 get_all_sync_states），
    // 使每文件的 skip 前身份校验与 decide_resume 都是内存查找，零额外 per-file IO。
    let resume_hints = resume_store
        .as_ref()
        .map(|s| s.load_all_sync_resume().unwrap_or_default())
        .unwrap_or_default();

    sync_progress::add_total(jsonl_files.len() as u32);

    for (file_path, file_mtime) in &jsonl_files {
        result.files_scanned += 1;

        match sync_single_file(
            db,
            file_path,
            *file_mtime,
            &sync_states,
            &mut pricing_cache,
            resume_store.as_ref(),
            &resume_hints,
        ) {
            Ok((imported, skipped)) => {
                result.imported += imported;
                result.skipped += skipped;
            }
            Err(e) => {
                let msg = format!("{}: {e}", file_path.display());
                log::warn!("[SESSION-SYNC] 文件解析失败: {msg}");
                result.errors.push(msg);
            }
        }
        sync_progress::add_done(1);
    }

    if result.imported > 0 {
        log::info!(
            "[SESSION-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, 扫描 {} 个文件",
            result.imported,
            result.skipped,
            result.files_scanned
        );
    }

    Ok(result)
}

/// 收集目录下所有 .jsonl 文件（含子 agent 文件），返回 `(路径, mtime 纳秒)`
/// 并按 mtime 降序排序（最近修改的文件最先返回）。
///
/// 扫描三层固定深度，不使用递归，避免死循环：
///   projects_dir/项目目录/*.jsonl                          (主会话)
///   projects_dir/项目目录/SESSION_ID/subagents/*.jsonl      (子 agent)
///
/// walk 阶段顺带取 mtime，既用于排序也传给后续 `sync_single_file`，避免二次
/// stat（无法读取 metadata 时记 0，交由 `sync_single_file` 回退处理）。
fn collect_jsonl_files(projects_dir: &Path) -> Vec<(PathBuf, i64)> {
    let mut files: Vec<(PathBuf, i64)> = Vec::new();

    let entries = match fs::read_dir(projects_dir) {
        Ok(e) => e,
        Err(_) => return files,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // 每个项目目录下的 .jsonl 文件
        if let Ok(sub_entries) = fs::read_dir(&path) {
            for sub_entry in sub_entries.flatten() {
                let sub_path = sub_entry.path();
                if sub_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    // 主会话 JSONL 文件
                    push_jsonl_file(&mut files, sub_path);
                } else if sub_path.is_dir() {
                    // 扫描子 agent 目录: 项目/SESSION_ID/subagents/*.jsonl
                    let subagents_dir = sub_path.join("subagents");
                    if subagents_dir.is_dir() {
                        if let Ok(agent_entries) = fs::read_dir(&subagents_dir) {
                            for agent_entry in agent_entries.flatten() {
                                let agent_path = agent_entry.path();
                                if agent_path.extension().and_then(|e| e.to_str()) == Some("jsonl")
                                {
                                    push_jsonl_file(&mut files, agent_path);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // mtime 降序：首次导入时最近的历史最先入库，Usage 默认 Today/7d 能尽快出数。
    files.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    files
}

/// 记录一个 jsonl 文件及其 mtime（读取失败记 0）。
fn push_jsonl_file(files: &mut Vec<(PathBuf, i64)>, path: PathBuf) {
    let mtime = fs::metadata(&path)
        .map(|m| metadata_modified_nanos(&m))
        .unwrap_or(0);
    files.push((path, mtime));
}

/// Claude 的驱动状态机：只需跨行携带 session id（序列化进 sidecar 提示）。
#[derive(Debug, Serialize, Deserialize)]
struct ClaudeResumeState {
    session_id: Option<String>,
}

enum ClaudeCursorResolution {
    SkipUnchanged,
    Continue {
        file: Option<File>,
        hint: Option<SyncResumeHint>,
    },
}

/// Convert upstream's legacy line cursor to the byte immediately after the
/// same `read_until` fragments, including a final fragment without `\n`.
fn legacy_cursor_hint(
    file: &mut File,
    file_path: &str,
    state: ClaudeSyncState,
) -> Result<SyncResumeHint, AppError> {
    let mut byte_offset = 0u64;
    {
        let mut reader = BufReader::new(&mut *file);
        let mut raw = Vec::new();
        for _ in 0..state.last_line_offset {
            raw.clear();
            let read = reader
                .read_until(b'\n', &mut raw)
                .map_err(|e| AppError::Config(format!("读取 Claude 旧行游标失败: {e}")))?;
            if read == 0 {
                break;
            }
            byte_offset += read as u64;
        }
    }

    let parser_state = serde_json::to_string(&ClaudeResumeState { session_id: None })
        .map_err(|e| AppError::Config(format!("序列化 Claude 续传状态失败: {e}")))?;
    resume_hint_from_shared_cursor(
        file,
        file_path,
        state.last_modified,
        state.last_line_offset,
        byte_offset,
        parser_state,
    )
    .ok_or_else(|| AppError::Config("无法转换 Claude 旧行游标".to_string()))
}

/// Resolve a legacy or v18 Claude cursor and retain the validated descriptor
/// for scanning, closing the validate/reopen race around path replacement.
fn resolve_claude_cursor(
    db: &Database,
    file_path: &Path,
    state: ClaudeSyncState,
    resume_line_offset: i64,
    matching_sidecar: Option<&SyncResumeHint>,
) -> Result<ClaudeCursorResolution, AppError> {
    let metadata = fs::metadata(file_path)
        .map_err(|e| AppError::Config(format!("无法读取文件元数据: {e}")))?;
    let file_modified = metadata_modified_nanos(&metadata);
    if file_modified <= state.last_modified {
        if state.last_byte_offset.is_none() {
            return Ok(ClaudeCursorResolution::SkipUnchanged);
        }
        let mut identity_hint = matching_sidecar.cloned();
        if let Some(hint) = identity_hint.as_mut() {
            hint.last_line_offset = resume_line_offset;
        }
        if !unchanged_jsonl_identity_is_suspicious(
            &metadata,
            identity_hint.as_ref(),
            state.last_modified,
            resume_line_offset,
        ) {
            return Ok(ClaudeCursorResolution::SkipUnchanged);
        }
    }

    let mut file =
        File::open(file_path).map_err(|e| AppError::Config(format!("无法打开文件: {e}")))?;
    let metadata = file
        .metadata()
        .map_err(|e| AppError::Config(format!("无法读取文件元数据: {e}")))?;
    let file_modified = metadata_modified_nanos(&metadata);
    let file_size = metadata.len();
    let Some(raw_offset) = state.last_byte_offset else {
        let hint = if state.last_line_offset > 0 {
            Some(legacy_cursor_hint(
                &mut file,
                &file_path.to_string_lossy(),
                state,
            )?)
        } else {
            None
        };
        return Ok(ClaudeCursorResolution::Continue {
            file: Some(file),
            hint,
        });
    };
    let byte_offset = u64::try_from(raw_offset).ok();
    let mut rewrite_reason = match byte_offset {
        Some(offset) if offset <= file_size => None,
        _ => Some("截断"),
    };

    if let Some(offset) = byte_offset.filter(|offset| *offset <= file_size) {
        if let Some(expected) = state.last_tail_fingerprint {
            let actual = shared_tail_fingerprint_from_file(&mut file, offset)
                .ok_or_else(|| AppError::Config("无法读取 Claude 游标边界尾部".to_string()))?;
            if actual != expected {
                rewrite_reason = Some("重写");
            }
        }
    }

    if let Some(reason) = rewrite_reason {
        let fingerprint = shared_tail_fingerprint_from_file(&mut file, file_size)
            .ok_or_else(|| AppError::Config("无法读取 Claude 文件尾部".to_string()))?;
        let pinned_offset = i64::try_from(file_size)
            .map_err(|_| AppError::Config("Claude 会话日志过大，无法保存字节游标".to_string()))?;
        let conn = lock_conn!(db.conn);
        update_claude_sync_state_conn(
            &conn,
            &file_path.to_string_lossy(),
            file_modified,
            pinned_offset,
            Some(fingerprint),
        )?;
        return Err(AppError::Config(format!(
            "检测到文件被外部{reason}，改写区间已跳过以防重复计数（不会再导入）"
        )));
    }

    let byte_offset = byte_offset.unwrap_or_default();
    if byte_offset == 0 {
        return Ok(ClaudeCursorResolution::Continue {
            file: Some(file),
            hint: None,
        });
    }
    let parser_state = serde_json::to_string(&ClaudeResumeState { session_id: None })
        .map_err(|e| AppError::Config(format!("序列化 Claude 续传状态失败: {e}")))?;
    let hint = resume_hint_from_shared_cursor(
        &mut file,
        &file_path.to_string_lossy(),
        state.last_modified,
        resume_line_offset,
        byte_offset,
        parser_state,
    )
    .ok_or_else(|| AppError::Config("无法建立 Claude 字节续传提示".to_string()))?;
    Ok(ClaudeCursorResolution::Continue {
        file: Some(file),
        hint: Some(hint),
    })
}

/// 同步单个 JSONL 文件，返回 (imported, skipped)
///
/// 文件读取走通用增量驱动（`session_usage_driver`）：mtime 跳过、sidecar
/// 字节续传、行 offset 回退都由驱动处理；本函数只负责 Claude 行解析与
/// 写库语义。
fn sync_single_file(
    db: &Database,
    file_path: &Path,
    file_mtime: i64,
    sync_states: &HashMap<String, ClaudeSyncState>,
    pricing_cache: &mut PricingCache,
    resume: Option<&ScanCacheStore>,
    resume_hints: &HashMap<String, SyncResumeHint>,
) -> Result<(u32, u32), AppError> {
    let file_path_str = file_path.to_string_lossy().to_string();

    // 检查同步状态（从预加载的快照读取，避免每个文件一次 DB 查询）
    let state = sync_states.get(&file_path_str).copied().unwrap_or_default();
    // 通用驱动只把非零行游标视为可续传。上游 v18 固定写 0，因此字节游标
    // 非零时仅在本轮驱动快照中使用 1；提交主库和 sidecar 前仍写回上游的 0。
    let driver_line_offset = if state.last_byte_offset.is_some_and(|offset| offset > 0) {
        state.last_line_offset.max(1)
    } else {
        state.last_line_offset
    };

    let mut messages: HashMap<String, ParsedAssistantUsage> = HashMap::new();

    // fix 2：续传提示取自预载 map（零 per-file 查询）；sidecar 是否可用另行传入，
    // 供驱动决定末行无换行时是否回退 mtime-1。
    let mut hint = resume_hints.get(&file_path_str).cloned();
    let sidecar_matches = hint.as_ref().is_some_and(|hint| {
        hint.last_modified == state.last_modified
            && hint.last_line_offset == state.last_line_offset
            && state
                .last_byte_offset
                .is_none_or(|offset| hint.byte_offset == offset)
    });
    let matching_sidecar = sidecar_matches.then_some(hint.as_ref()).flatten();
    let scan_file;
    match resolve_claude_cursor(db, file_path, state, driver_line_offset, matching_sidecar)? {
        ClaudeCursorResolution::SkipUnchanged => return Ok((0, 0)),
        ClaudeCursorResolution::Continue {
            file,
            hint: Some(mut resolved_hint),
        } => {
            scan_file = file;
            if state.last_byte_offset.is_some() && sidecar_matches {
                let local_hint = hint.as_ref().expect("matching hint must exist");
                let local_state_is_valid = local_hint
                    .state
                    .as_deref()
                    .is_some_and(|state| serde_json::from_str::<ClaudeResumeState>(state).is_ok());
                if local_state_is_valid {
                    resolved_hint.state.clone_from(&local_hint.state);
                    resolved_hint.pending_tail_len = local_hint.pending_tail_len;
                    resolved_hint.pending_tail_hash = local_hint.pending_tail_hash;
                }
            }
            hint = Some(resolved_hint);
        }
        ClaudeCursorResolution::Continue { file, hint: None } => {
            scan_file = file;
            if state.last_byte_offset.is_none() || !sidecar_matches {
                hint = None;
            }
        }
    }

    let outcome = scan_jsonl_incremental(
        file_path,
        scan_file,
        file_mtime,
        state.last_modified,
        driver_line_offset,
        hint,
        resume.is_some(),
        || ClaudeResumeState { session_id: None },
        |state, line, is_new| {
            // Claude 无需重放历史行重建状态（session id 存在提示里），
            // 回退路径的旧行直接跳过。
            if !is_new {
                return;
            }

            // 预过滤：session id 已确定且该行不含 assistant 标记时，直接跳过
            // 不解析，避免为多兆字节的 tool_result 行构建结构。session id 未
            // 确定前的行仍需解析（首行通常携带 sessionId）。
            if state.session_id.is_some() && !line.contains("\"assistant\"") {
                return;
            }

            let parsed: NarrowClaudeLine = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => return,
            };

            // 提取 session ID (从 system 或首条消息)
            if state.session_id.is_none() {
                if let Some(sid) = parsed.session_id.as_deref() {
                    state.session_id = Some(sid.to_string());
                }
            }

            // 只处理 assistant 类型的消息
            if parsed.kind.as_deref() != Some("assistant") {
                return;
            }

            let Some(message) = parsed.message else {
                return;
            };
            let Some(msg_id) = message.id else {
                return;
            };
            let Some(usage) = message.usage else {
                return;
            };

            let parsed_usage = ParsedAssistantUsage {
                message_id: msg_id.clone(),
                model: message.model.unwrap_or_else(|| "unknown".to_string()),
                input_tokens: usage.input_tokens.unwrap_or(0) as u32,
                output_tokens: usage.output_tokens.unwrap_or(0) as u32,
                cache_read_tokens: usage.cache_read_input_tokens.unwrap_or(0) as u32,
                cache_creation_tokens: usage.cache_creation_input_tokens.unwrap_or(0) as u32,
                stop_reason: message.stop_reason,
                timestamp: parsed.timestamp,
                session_id: state.session_id.clone(),
            };

            // 按 message.id 去重：优先保留有 stop_reason 的条目，否则保留最新的
            let should_replace = match messages.get(&msg_id) {
                None => true,
                Some(existing) => {
                    // 新条目有 stop_reason 而旧条目没有 → 替换
                    if parsed_usage.stop_reason.is_some() && existing.stop_reason.is_none() {
                        true
                    }
                    // 两个都有或都没有 stop_reason → 取 output_tokens 更大的
                    else if parsed_usage.stop_reason.is_some() == existing.stop_reason.is_some() {
                        parsed_usage.output_tokens > existing.output_tokens
                    } else {
                        false
                    }
                }
            };

            if should_replace {
                messages.insert(msg_id, parsed_usage);
            }
        },
    )?;

    // 文件未变化（mtime 跳过）
    let Some(mut outcome) = outcome else {
        return Ok((0, 0));
    };
    let shared_tail_fingerprint = outcome
        .shared_tail_fingerprint
        .ok_or_else(|| AppError::Config("无法计算 Claude 游标边界指纹".to_string()))?;

    // 写入数据库：整文件在一个事务内完成 INSERT / 去重查询 / 同步状态更新，
    // 超大文件每 SESSION_LOG_COMMIT_BATCH 行分段提交，避免逐行 fsync。
    let mut imported: u32 = 0;
    let mut skipped: u32 = 0;

    let mut guard = lock_conn!(db.conn);
    let mut tx = guard
        .transaction()
        .map_err(|e| AppError::Database(format!("开启事务失败: {e}")))?;
    let mut since_commit: u32 = 0;

    for msg in messages.values() {
        // 只导入有 stop_reason 的最终条目（完整的 API 调用）
        if msg.stop_reason.is_none() {
            continue;
        }

        // 跳过 output_tokens 为 0 的无意义条目
        if msg.output_tokens == 0 {
            continue;
        }

        let request_id = format!(
            "{}{}",
            crate::proxy::usage::parser::SESSION_REQUEST_ID_PREFIX,
            msg.message_id
        );

        match insert_session_log_entry(&tx, pricing_cache, &request_id, msg) {
            Ok(true) => imported += 1,
            Ok(false) => skipped += 1,
            Err(e) => {
                log::warn!("[SESSION-SYNC] 插入失败 ({}): {e}", msg.message_id);
                skipped += 1;
            }
        }

        since_commit += 1;
        if since_commit >= SESSION_LOG_COMMIT_BATCH {
            tx.commit()
                .map_err(|e| AppError::Database(format!("提交事务失败: {e}")))?;
            tx = guard
                .transaction()
                .map_err(|e| AppError::Database(format!("开启事务失败: {e}")))?;
            since_commit = 0;
        }
    }

    // 在同一事务内更新同步状态后统一提交
    let byte_offset = i64::try_from(outcome.byte_pos)
        .map_err(|_| AppError::Config("Claude 会话日志过大，无法保存字节游标".to_string()))?;
    update_claude_sync_state_conn(
        &tx,
        &file_path_str,
        outcome.file_modified,
        byte_offset,
        Some(shared_tail_fingerprint),
    )?;
    tx.commit()
        .map_err(|e| AppError::Database(format!("提交事务失败: {e}")))?;
    drop(guard);

    // 与上游一致，字节游标建立后共享行号固定为 0；sidecar 也使用相同快照值。
    outcome.line_offset = 0;
    // 主库进度提交成功后，把字节位置与状态写回 sidecar（尽力而为）
    save_resume_hint(resume, &file_path_str, &outcome);

    Ok((imported, skipped))
}

/// 获取 session_log_sync 表中某条目的同步进度。
///
/// Shared by all session_usage_* parsers.
pub(crate) fn get_sync_state(db: &Database, file_path: &str) -> Result<(i64, i64), AppError> {
    let conn = lock_conn!(db.conn);
    let result = conn.query_row(
        "SELECT last_modified, last_line_offset FROM session_log_sync WHERE file_path = ?1",
        rusqlite::params![file_path],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    );
    Ok(result.unwrap_or((0, 0)))
}

/// Load the entire `session_log_sync` table in one query as
/// `file_path -> (last_modified, last_line_offset)`. Lets a provider with tens
/// of thousands of session files check sync state from memory instead of
/// issuing one `get_sync_state` query per file.
pub(crate) fn get_all_sync_states(db: &Database) -> Result<HashMap<String, (i64, i64)>, AppError> {
    let conn = lock_conn!(db.conn);
    let mut states = HashMap::new();
    // Tolerate read errors the same way the old per-file `get_sync_state` did
    // (it returned (0,0) on failure): a missing/unreadable entry just means that
    // file is treated as never-synced and re-parsed, rather than failing the
    // whole sync.
    let mut stmt = match conn
        .prepare("SELECT file_path, last_modified, last_line_offset FROM session_log_sync")
    {
        Ok(stmt) => stmt,
        Err(e) => {
            log::warn!("[SESSION-SYNC] 读取同步状态失败，将按未同步重扫: {e}");
            return Ok(states);
        }
    };
    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
        ))
    }) {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("[SESSION-SYNC] 读取同步状态失败，将按未同步重扫: {e}");
            return Ok(states);
        }
    };
    for row in rows {
        match row {
            Ok((file_path, state)) => {
                states.insert(file_path, state);
            }
            Err(e) => log::warn!("[SESSION-SYNC] 跳过损坏的同步状态行: {e}"),
        }
    }
    Ok(states)
}

/// 返回文件 mtime 的纳秒时间戳。
///
/// `session_log_sync.last_modified` 旧数据是秒级时间戳；新写入纳秒值不需要
/// schema 迁移，旧值会自然触发一次增量重扫，并继续依赖行 offset 避免重复导入。
pub(crate) fn metadata_modified_nanos(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// 更新 session_log_sync 表中某条目的同步进度（连接版本）。
///
/// 供批量事务复用：调用方已持有事务连接，直接在同一事务内写入同步状态。
pub(crate) fn update_sync_state_conn(
    conn: &rusqlite::Connection,
    file_path: &str,
    last_modified: i64,
    last_offset: i64,
) -> Result<(), AppError> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // 单调更新：并发同步（TUI/daemon/proxy/CLI 可能同时运行）中较晚提交的
    // 旧快照不得把进度倒退回去。文件系统 mtime 粒度有限，同一 tick 内两个
    // 快照的 mtime 可能相等，因此按 (mtime, line_offset) 字典序判定：
    // mtime 更新才整体覆盖；mtime 相等时只允许 offset 不回退。
    conn.execute(
        "INSERT INTO session_log_sync (file_path, last_modified, last_line_offset, last_synced_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(file_path) DO UPDATE SET
            last_modified = excluded.last_modified,
            last_line_offset = excluded.last_line_offset,
            last_synced_at = excluded.last_synced_at
         WHERE excluded.last_modified > session_log_sync.last_modified
            OR (excluded.last_modified = session_log_sync.last_modified
                AND excluded.last_line_offset >= session_log_sync.last_line_offset)",
        rusqlite::params![file_path, last_modified, last_offset, now],
    )
    .map_err(|e| AppError::Database(format!("更新同步状态失败: {e}")))?;
    Ok(())
}

/// Write the shared v18 Claude cursor using upstream's byte-offset,
/// fingerprint, and fixed-zero line-offset semantics.
fn update_claude_sync_state_conn(
    conn: &rusqlite::Connection,
    file_path: &str,
    last_modified: i64,
    last_byte_offset: i64,
    last_tail_fingerprint: Option<i64>,
) -> Result<(), AppError> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    conn.prepare_cached(
        "INSERT OR REPLACE INTO session_log_sync
             (file_path, last_modified, last_line_offset, last_synced_at, last_byte_offset,
              last_tail_fingerprint)
         VALUES (?1, ?2, 0, ?3, ?4, ?5)",
    )
    .and_then(|mut stmt| {
        stmt.execute(rusqlite::params![
            file_path,
            last_modified,
            now,
            last_byte_offset,
            last_tail_fingerprint,
        ])
    })
    .map_err(|e| AppError::Database(format!("更新 Claude 同步状态失败: {e}")))?;
    Ok(())
}

/// Update one session cursor outside an existing transaction.
pub(crate) fn update_sync_state(
    db: &Database,
    file_path: &str,
    last_modified: i64,
    last_offset: i64,
) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    update_sync_state_conn(&conn, file_path, last_modified, last_offset)
}

/// 插入单条会话日志到 proxy_request_logs，返回是否成功插入 (true=新插入, false=已存在)
///
/// 调用方在同一事务连接上批量调用本函数；INSERT 与去重查询均走 prepare_cached
/// 复用编译结果，费用查询走 per-cycle 定价缓存。
fn insert_session_log_entry(
    conn: &rusqlite::Connection,
    pricing_cache: &mut PricingCache,
    request_id: &str,
    msg: &ParsedAssistantUsage,
) -> Result<bool, AppError> {
    let created_at = msg
        .timestamp
        .as_ref()
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
        });

    let dedup_key = DedupKey {
        app_type: "claude",
        model: &msg.model,
        input_tokens: msg.input_tokens,
        output_tokens: msg.output_tokens,
        cache_read_tokens: msg.cache_read_tokens,
        cache_creation_tokens: msg.cache_creation_tokens,
        created_at,
    };
    if should_skip_session_insert(conn, request_id, &dedup_key)? {
        return Ok(false);
    }

    // 计算费用
    let usage = TokenUsage {
        input_tokens: msg.input_tokens,
        output_tokens: msg.output_tokens,
        cache_read_tokens: msg.cache_read_tokens,
        cache_creation_tokens: msg.cache_creation_tokens,
        model: Some(msg.model.clone()),
        message_id: None,
    };

    let pricing = cached_model_pricing(conn, pricing_cache, &msg.model);
    let pricing_model = if pricing.is_some() {
        msg.model.as_str()
    } else {
        ""
    };
    let multiplier = Decimal::from(1);
    let (input_cost, output_cost, cache_read_cost, cache_creation_cost, total_cost) = match pricing
    {
        Some(p) => {
            let cost = CostCalculator::calculate(&usage, &p, multiplier);
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
        .map_err(|e| AppError::Database(format!("插入会话日志失败: {e}")))?;
    let inserted_rows = stmt
        .execute(rusqlite::params![
            request_id,
            "_session", // provider_id: 标记为会话来源
            "claude",   // app_type
            msg.model,
            msg.model, // request_model = model
            pricing_model,
            msg.input_tokens,
            msg.output_tokens,
            msg.cache_read_tokens,
            msg.cache_creation_tokens,
            input_cost,
            output_cost,
            cache_read_cost,
            cache_creation_cost,
            total_cost,
            0i64,                   // latency_ms: 会话日志无此数据
            Option::<i64>::None,    // first_token_ms
            200i64,                 // status_code: 有 stop_reason 说明请求成功
            Option::<String>::None, // error_message
            msg.session_id,
            Some("session_log"), // provider_type
            1i64,                // is_streaming: Claude Code 通常使用流式
            "1.0",               // cost_multiplier
            created_at,
            "session_log", // data_source
        ])
        .map_err(|e| AppError::Database(format!("插入会话日志失败: {e}")))?;

    // INSERT OR IGNORE 被并发进程抢先时未写入行，计为 skipped 而非 imported
    Ok(inserted_rows > 0)
}

/// 查询数据来源分布统计
#[allow(dead_code)]
pub fn get_data_source_breakdown(db: &Database) -> Result<Vec<DataSourceSummary>, AppError> {
    let conn = lock_conn!(db.conn);

    let effective_filter = effective_usage_log_filter("l");
    let sql = format!(
        "SELECT COALESCE(l.data_source, 'proxy') as ds, COUNT(*) as cnt,
                COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0) as cost
         FROM proxy_request_logs l
         WHERE {effective_filter}
         GROUP BY ds
         ORDER BY cnt DESC"
    );

    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map([], |row| {
        Ok(DataSourceSummary {
            data_source: row.get(0)?,
            request_count: row.get::<_, i64>(1)? as u32,
            total_cost_usd: format!("{:.6}", row.get::<_, f64>(2)?),
        })
    })?;

    let mut summaries = Vec::new();
    for row in rows {
        summaries.push(row.map_err(|e| AppError::Database(e.to_string()))?);
    }

    Ok(summaries)
}

pub(crate) fn delete_session_logs_covered_by_proxy_log(
    conn: &rusqlite::Connection,
    app_type: &str,
    model: &str,
    usage: &TokenUsage,
    created_at: i64,
) -> Result<usize, AppError> {
    if usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.cache_read_tokens == 0
        && usage.cache_creation_tokens == 0
    {
        return Ok(0);
    }

    conn.execute(
        "DELETE FROM proxy_request_logs
         WHERE COALESCE(data_source, 'proxy') IN ('session_log', 'codex_session', 'gemini_session', 'opencode_session')
           AND app_type = ?1
           AND status_code >= 200
           AND status_code < 300
           AND input_tokens = ?3
           AND output_tokens = ?4
           AND cache_read_tokens = ?5
           AND (
               cache_creation_tokens = ?6
               OR (
                   cache_creation_tokens = 0
                   AND COALESCE(data_source, 'proxy') IN ('codex_session', 'gemini_session', 'opencode_session')
               )
           )
           AND created_at BETWEEN ?7 - ?8 AND ?7 + ?8
           AND (
               LOWER(model) = LOWER(?2)
               OR LOWER(model) = 'unknown'
               OR LOWER(?2) = 'unknown'
           )",
        rusqlite::params![
            app_type,
            model,
            usage.input_tokens as i64,
            usage.output_tokens as i64,
            usage.cache_read_tokens as i64,
            usage.cache_creation_tokens as i64,
            created_at,
            crate::services::usage_stats::SESSION_PROXY_DEDUP_WINDOW_SECONDS,
        ],
    )
    .map_err(|error| AppError::Database(format!("删除重复 session 用量日志失败: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_database_session_guard_does_not_create_a_lock_file() -> Result<(), AppError> {
        let home = tempfile::tempdir().expect("isolated test home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let db = Database::memory()?;
        assert!(db.session_usage_lock_path().is_none());
        let _guard = acquire_session_sync_guard(&db)?;
        assert!(!home.path().join(".cc-switch").exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn session_sync_lock_rejects_symlinks_and_hardlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("target");
        fs::write(&target, b"sentinel").expect("seed target");

        let symlink_path = temp.path().join("symlink.lock");
        symlink(&target, &symlink_path).expect("create symlink");
        assert!(open_session_sync_lock(&symlink_path).is_err());

        let hardlink_path = temp.path().join("hardlink.lock");
        fs::hard_link(&target, &hardlink_path).expect("create hardlink");
        assert!(open_session_sync_lock(&hardlink_path).is_err());
        assert_eq!(fs::read(&target).expect("read target"), b"sentinel");
    }

    #[test]
    fn session_sync_guard_blocks_another_process_and_keeps_lock_inode() -> Result<(), AppError> {
        const CHILD_ENV: &str = "CC_SWITCH_TEST_SESSION_LOCK_CHILD";
        const TEST_NAME: &str = "services::session_usage::tests::session_sync_guard_blocks_another_process_and_keeps_lock_inode";

        if let Some(home) = std::env::var_os(CHILD_ENV) {
            let home = PathBuf::from(home);
            let _env = crate::test_support::TestEnvGuard::isolated(&home);
            let db = Database::init()?;
            fs::write(home.join("attempted"), b"1").expect("write attempted marker");
            let _guard = acquire_session_sync_guard(&db)?;
            fs::write(home.join("acquired"), b"1").expect("write acquired marker");
            return Ok(());
        }

        let home = tempfile::tempdir().expect("isolated test home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let db = Database::init()?;
        let lock_path = db.session_usage_lock_path().expect("file database lock");
        let guard = acquire_session_sync_guard(&db)?;

        let mut child = std::process::Command::new(
            std::env::current_exe().expect("resolve current test executable"),
        )
        .args(["--exact", TEST_NAME, "--test-threads=1"])
        .env(CHILD_ENV, home.path())
        .spawn()
        .expect("spawn lock contender");

        let attempted = home.path().join("attempted");
        let acquired = home.path().join("acquired");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !attempted.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(attempted.exists(), "child did not reach lock acquisition");
        std::thread::sleep(Duration::from_millis(150));
        assert!(!acquired.exists(), "child bypassed the held file lock");

        drop(guard);
        let status = child.wait().expect("wait for lock contender");
        assert!(status.success(), "lock contender failed: {status}");
        assert!(acquired.exists());
        assert!(lock_path.exists(), "lock file must keep a stable inode");
        Ok(())
    }

    /// sync_progress：begin 归零并置 active，guard drop 后 snapshot 归 None。
    /// （lib 测试内只有本用例读写这些计数器：单文件级 sync_* 测试不经过
    /// 外层循环的埋点，不会并发干扰。）
    #[test]
    fn sync_progress_guard_scopes_snapshot() {
        assert!(sync_progress::snapshot().is_none());
        {
            let _guard = sync_progress::begin();
            sync_progress::add_total(3);
            sync_progress::add_done(1);
            assert_eq!(sync_progress::snapshot(), Some((1, 3)));
        }
        assert!(sync_progress::snapshot().is_none());
    }

    #[test]
    fn claude_session_import_records_explicit_pricing_evidence() -> Result<(), AppError> {
        let db = Database::memory()?;
        let conn = lock_conn!(db.conn);
        conn.execute(
            "INSERT OR REPLACE INTO model_pricing (
                 model_id, display_name, input_cost_per_million,
                 output_cost_per_million, cache_read_cost_per_million,
                 cache_creation_cost_per_million
             ) VALUES ('writer-priced-free', 'Writer Priced Free', '0', '0', '0', '0')",
            [],
        )?;

        let mut pricing_cache = PricingCache::new();
        for (request_id, model, timestamp) in [
            (
                "claude-priced-evidence",
                "writer-priced-free",
                "1970-01-01T00:16:40Z",
            ),
            (
                "claude-unpriced-evidence",
                "writer-unknown",
                "1970-01-01T00:33:20Z",
            ),
        ] {
            let message = ParsedAssistantUsage {
                message_id: request_id.to_string(),
                model: model.to_string(),
                input_tokens: 10,
                output_tokens: 1,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                stop_reason: Some("end_turn".to_string()),
                timestamp: Some(timestamp.to_string()),
                session_id: Some(request_id.to_string()),
            };
            assert!(insert_session_log_entry(
                &conn,
                &mut pricing_cache,
                request_id,
                &message
            )?);
        }

        let priced: Option<String> = conn.query_row(
            "SELECT pricing_model FROM proxy_request_logs
             WHERE request_id = 'claude-priced-evidence'",
            [],
            |row| row.get(0),
        )?;
        let unpriced: Option<String> = conn.query_row(
            "SELECT pricing_model FROM proxy_request_logs
             WHERE request_id = 'claude-unpriced-evidence'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(priced.as_deref(), Some("writer-priced-free"));
        assert_eq!(
            unpriced.as_deref(),
            Some(""),
            "new unpriced rows must be explicit rather than legacy NULL"
        );

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn codex_rebuild_backup_failure_keeps_existing_usage() -> Result<(), AppError> {
        use std::os::unix::fs::symlink;

        let home = tempfile::tempdir().expect("isolated test home");
        let _env = crate::test_support::TestEnvGuard::isolated(home.path());
        let db = Database::init()?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, input_tokens,
                    output_tokens, cache_read_tokens, latency_ms, status_code,
                    created_at, data_source
                 ) VALUES
                    ('codex-before-failed-backup', '_codex_session', 'codex', 'gpt',
                     1, 1, 0, 0, 200, 1, 'codex_session');",
            )?;
        }

        let backup_dir = home.path().join(".cc-switch/backups");
        if backup_dir.exists() {
            fs::remove_dir_all(&backup_dir).expect("remove existing backup directory");
        }
        let outside = home.path().join("outside-backups");
        fs::create_dir_all(&outside).expect("create external backup target");
        symlink(&outside, &backup_dir).expect("create backup directory symlink");

        assert!(rebuild_codex_usage(&db).is_err());
        let conn = lock_conn!(db.conn);
        let remaining: i64 = conn.query_row(
            "SELECT COUNT(*) FROM proxy_request_logs
             WHERE request_id = 'codex-before-failed-backup'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(remaining, 1);
        Ok(())
    }

    #[test]
    fn test_parse_usage_from_jsonl_line() {
        let line = r#"{"type":"assistant","message":{"id":"msg_test123","model":"claude-opus-4-6","usage":{"input_tokens":3,"output_tokens":150,"cache_read_input_tokens":5000,"cache_creation_input_tokens":10000},"stop_reason":"end_turn"},"timestamp":"2026-04-05T12:00:00Z","sessionId":"session-abc"}"#;

        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            value.get("type").and_then(|t| t.as_str()),
            Some("assistant")
        );

        let message = value.get("message").unwrap();
        let usage = message.get("usage").unwrap();

        assert_eq!(usage.get("input_tokens").unwrap().as_u64().unwrap(), 3);
        assert_eq!(usage.get("output_tokens").unwrap().as_u64().unwrap(), 150);
        assert_eq!(
            usage
                .get("cache_read_input_tokens")
                .unwrap()
                .as_u64()
                .unwrap(),
            5000
        );
        assert_eq!(
            usage
                .get("cache_creation_input_tokens")
                .unwrap()
                .as_u64()
                .unwrap(),
            10000
        );
        assert_eq!(
            message.get("stop_reason").unwrap().as_str().unwrap(),
            "end_turn"
        );
    }

    #[test]
    fn test_dedup_by_message_id() {
        // 同一个 message.id 有多条，应该取 stop_reason 有值的那条
        let mut messages: HashMap<String, ParsedAssistantUsage> = HashMap::new();

        // 中间条目（无 stop_reason）
        let intermediate = ParsedAssistantUsage {
            message_id: "msg_1".to_string(),
            model: "claude-opus-4-6".to_string(),
            input_tokens: 3,
            output_tokens: 26,
            cache_read_tokens: 5000,
            cache_creation_tokens: 10000,
            stop_reason: None,
            timestamp: Some("2026-04-05T12:00:00Z".to_string()),
            session_id: None,
        };
        messages.insert("msg_1".to_string(), intermediate);

        // 最终条目（有 stop_reason）
        let final_entry = ParsedAssistantUsage {
            message_id: "msg_1".to_string(),
            model: "claude-opus-4-6".to_string(),
            input_tokens: 3,
            output_tokens: 1349,
            cache_read_tokens: 5000,
            cache_creation_tokens: 10000,
            stop_reason: Some("end_turn".to_string()),
            timestamp: Some("2026-04-05T12:00:00Z".to_string()),
            session_id: None,
        };

        // 应该替换
        let should_replace = final_entry.stop_reason.is_some()
            && messages.get("msg_1").unwrap().stop_reason.is_none();
        assert!(should_replace);

        messages.insert("msg_1".to_string(), final_entry);
        assert_eq!(messages.get("msg_1").unwrap().output_tokens, 1349);
    }

    #[test]
    fn test_insert_claude_session_skips_matching_proxy_log() -> Result<(), AppError> {
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
                    "proxy-different-id",
                    "openai-compatible",
                    "claude",
                    "claude-sonnet-4-5",
                    "claude-sonnet-4-5",
                    100,
                    20,
                    10,
                    5,
                    "0.10",
                    100,
                    200,
                    1000,
                    "proxy"
                ],
            )?;
        }

        let msg = ParsedAssistantUsage {
            message_id: "msg_1".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 10,
            cache_creation_tokens: 5,
            stop_reason: Some("end_turn".to_string()),
            timestamp: Some("1970-01-01T00:16:45Z".to_string()),
            session_id: Some("session-1".to_string()),
        };

        let mut pricing_cache = PricingCache::new();
        let inserted = {
            let conn = lock_conn!(db.conn);
            insert_session_log_entry(&conn, &mut pricing_cache, "session:msg_1", &msg)?
        };
        assert!(!inserted);

        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
            row.get(0)
        })?;
        assert_eq!(count, 1);

        Ok(())
    }

    #[test]
    fn test_collect_jsonl_files_includes_subagents() {
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        let project = tmp.join("project");
        let session_dir = project.join("test-session");
        let subagents_dir = session_dir.join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();

        fs::write(project.join("main.jsonl"), "{}").unwrap();
        fs::write(subagents_dir.join("agent-abc.jsonl"), "{}").unwrap();

        let files = collect_jsonl_files(&tmp);
        assert_eq!(files.len(), 2);
        let paths: Vec<String> = files
            .iter()
            .map(|(p, _mtime)| p.to_string_lossy().to_string())
            .collect();
        assert!(paths.iter().any(|p| p.contains("main.jsonl")));
        assert!(paths.iter().any(|p| p.contains("agent-abc.jsonl")));

        fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn periodic_session_sync_tick_runs_cost_backfill_cycle() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create temp home");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let db = Arc::new(Database::memory()?);

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd,
                    total_cost_usd, latency_ms, status_code, created_at, data_source
                ) VALUES (
                    'periodic-backfill-zero-cost', '_codex_session', 'codex', 'gpt-5.5', 'gpt-5.5',
                    1000000, 0, 0, 0,
                    '0', '0', '0', '0',
                    '0', 100, 200, 1000, 'codex_session'
                )",
                [],
            )?;
        }

        run_periodic_session_usage_sync_tick_on_blocking_thread(
            db.clone(),
            "test-periodic".to_string(),
        )
        .await;

        let conn = lock_conn!(db.conn);
        let total_cost: String = conn.query_row(
            "SELECT total_cost_usd
             FROM proxy_request_logs
             WHERE request_id = 'periodic-backfill-zero-cost'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(total_cost, "5.000000");

        Ok(())
    }

    /// 进度写入按 (mtime, offset) 字典序单调：并发同步中较晚提交的旧快照
    /// 不得把进度倒退回去（mtime 粒度有限，相等时比较 offset）。
    #[test]
    fn sync_state_updates_are_monotonic() -> Result<(), AppError> {
        let db = Database::memory()?;
        {
            let conn = lock_conn!(db.conn);
            update_sync_state_conn(&conn, "/f.jsonl", 5, 120)?;
            // 同 mtime、更小 offset：旧快照，拒绝
            update_sync_state_conn(&conn, "/f.jsonl", 5, 100)?;
            // 更旧 mtime：拒绝
            update_sync_state_conn(&conn, "/f.jsonl", 4, 999)?;
        }
        assert_eq!(get_sync_state(&db, "/f.jsonl")?, (5, 120));

        {
            let conn = lock_conn!(db.conn);
            // 同 mtime、更大 offset：接受
            update_sync_state_conn(&conn, "/f.jsonl", 5, 130)?;
        }
        assert_eq!(get_sync_state(&db, "/f.jsonl")?, (5, 130));

        {
            let conn = lock_conn!(db.conn);
            // 更新的 mtime：整体覆盖（offset 允许变小，如文件重写后重扫）
            update_sync_state_conn(&conn, "/f.jsonl", 6, 10)?;
        }
        assert_eq!(get_sync_state(&db, "/f.jsonl")?, (6, 10));

        Ok(())
    }

    #[test]
    fn shared_v18_cursor_resumes_without_reimporting_prefix() -> Result<(), AppError> {
        let db = Database::memory()?;
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = r#"{"type":"assistant","message":{"id":"shared-a","model":"claude-x","usage":{"input_tokens":10,"output_tokens":100},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#;
        let second = r#"{"type":"assistant","message":{"id":"shared-b","model":"claude-x","usage":{"input_tokens":11,"output_tokens":200},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:01Z","sessionId":"s1"}"#;
        let third = r#"{"type":"assistant","message":{"id":"shared-c","model":"claude-x","usage":{"input_tokens":12,"output_tokens":300},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:02Z","sessionId":"s1"}"#;
        let prefix = format!("{first}\n{second}\n");
        let path = write_temp_jsonl(
            tmp.path(),
            "shared-v18.jsonl",
            &format!("{prefix}{third}\n"),
        );
        let path_str = path.to_string_lossy().to_string();

        let prefix_offset = prefix.len() as u64;
        let prefix_fingerprint = {
            let mut file = File::open(&path).expect("open shared cursor file");
            shared_tail_fingerprint_from_file(&mut file, prefix_offset).expect("fingerprint prefix")
        };
        let mut states = HashMap::new();
        states.insert(
            path_str.clone(),
            ClaudeSyncState {
                last_modified: 1,
                last_line_offset: 0,
                last_byte_offset: Some(prefix_offset as i64),
                last_tail_fingerprint: Some(prefix_fingerprint),
            },
        );
        let mut stale_sidecar = {
            let mut file = File::open(&path).expect("open stale sidecar file");
            resume_hint_from_shared_cursor(
                &mut file,
                &path_str,
                1,
                0,
                prefix_offset,
                serde_json::to_string(&ClaudeResumeState { session_id: None })
                    .expect("serialize state"),
            )
            .expect("build stale sidecar hint")
        };
        stale_sidecar.state = Some("not-json".to_string());
        let hints = HashMap::from([(path_str.clone(), stale_sidecar)]);

        let mut pricing_cache = PricingCache::new();
        let result = sync_single_file(&db, &path, 2, &states, &mut pricing_cache, None, &hints)?;
        assert_eq!(result, (1, 0));

        let conn = lock_conn!(db.conn);
        let imported: Vec<String> = conn
            .prepare(
                "SELECT request_id FROM proxy_request_logs
                 WHERE request_id IN ('session:shared-a', 'session:shared-b', 'session:shared-c')
                 ORDER BY request_id",
            )?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        assert_eq!(imported, vec!["session:shared-c"]);
        let cursor: (i64, i64, Option<i64>) = conn.query_row(
            "SELECT last_line_offset, last_byte_offset, last_tail_fingerprint
             FROM session_log_sync WHERE file_path = ?1",
            [&path_str],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(cursor.0, 0);
        assert_eq!(
            cursor.1,
            fs::metadata(&path).expect("stat shared cursor file").len() as i64
        );
        assert!(cursor.2.is_some());
        Ok(())
    }

    #[test]
    fn legacy_line_cursor_converts_without_reimporting_prefix() -> Result<(), AppError> {
        let db = Database::memory()?;
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = r#"{"type":"assistant","message":{"id":"legacy-a","model":"claude-x","usage":{"input_tokens":10,"output_tokens":100},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#;
        let replacement = first.replace("legacy-a", "legacy-x");
        assert_eq!(first.len(), replacement.len());
        let second = r#"{"type":"assistant","message":{"id":"legacy-b","model":"claude-x","usage":{"input_tokens":11,"output_tokens":200},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:01Z","sessionId":"s1"}"#;
        let path = write_temp_jsonl(
            tmp.path(),
            "legacy-v17.jsonl",
            &format!("{first}\n{second}\n"),
        );
        let path_str = path.to_string_lossy().to_string();
        let stale_sidecar = {
            let mut file = File::open(&path).expect("open legacy file");
            resume_hint_from_shared_cursor(
                &mut file,
                &path_str,
                1,
                1,
                (first.len() + 1) as u64,
                serde_json::to_string(&ClaudeResumeState { session_id: None })
                    .expect("serialize state"),
            )
            .expect("build legacy sidecar")
        };
        fs::write(&path, format!("{replacement}\n{second}\n")).expect("rewrite legacy prefix");
        let states = HashMap::from([(
            path_str.clone(),
            ClaudeSyncState {
                last_modified: 1,
                last_line_offset: 1,
                last_byte_offset: None,
                last_tail_fingerprint: None,
            },
        )]);
        let hints = HashMap::from([(path_str.clone(), stale_sidecar)]);

        let mut pricing_cache = PricingCache::new();
        assert_eq!(
            sync_single_file(&db, &path, 2, &states, &mut pricing_cache, None, &hints,)?,
            (1, 0)
        );

        let conn = lock_conn!(db.conn);
        let imported: Vec<String> = conn
            .prepare(
                "SELECT request_id FROM proxy_request_logs
                 WHERE request_id IN ('session:legacy-a', 'session:legacy-b', 'session:legacy-x')
                 ORDER BY request_id",
            )?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        assert_eq!(imported, vec!["session:legacy-b"]);
        let cursor: (i64, i64, Option<i64>) = conn.query_row(
            "SELECT last_line_offset, last_byte_offset, last_tail_fingerprint
             FROM session_log_sync WHERE file_path = ?1",
            [&path_str],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(cursor.0, 0);
        assert_eq!(
            cursor.1,
            fs::metadata(&path).expect("stat legacy file").len() as i64
        );
        assert!(cursor.2.is_some());
        Ok(())
    }

    #[test]
    fn legacy_cursor_includes_unterminated_line_before_future_append() -> Result<(), AppError> {
        let db = Database::memory()?;
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = test_claude_line("legacy-tail-a", 100);
        let second = test_claude_line("legacy-tail-b", 200);
        let third = test_claude_line("legacy-tail-c", 300);
        let path = write_temp_jsonl(
            tmp.path(),
            "legacy-unterminated.jsonl",
            &format!("{first}\n{second}"),
        );
        let path_str = path.to_string_lossy().to_string();
        let states = HashMap::from([(
            path_str.clone(),
            ClaudeSyncState {
                last_modified: 1,
                last_line_offset: 2,
                last_byte_offset: None,
                last_tail_fingerprint: None,
            },
        )]);

        let mut pricing_cache = PricingCache::new();
        assert_eq!(
            sync_single_file(
                &db,
                &path,
                2,
                &states,
                &mut pricing_cache,
                None,
                &HashMap::new(),
            )?,
            (0, 0)
        );
        {
            let conn = lock_conn!(db.conn);
            let converted_offset: i64 = conn.query_row(
                "SELECT last_byte_offset FROM session_log_sync WHERE file_path = ?1",
                [&path_str],
                |row| row.get(0),
            )?;
            assert_eq!(converted_offset, (first.len() + 1 + second.len()) as i64);
        }

        fs::write(&path, format!("{first}\n{second}\n{third}\n"))
            .expect("append after unterminated legacy line");
        File::options()
            .write(true)
            .open(&path)
            .expect("open appended legacy file")
            .set_times(
                fs::FileTimes::new().set_modified(SystemTime::now() + Duration::from_secs(2)),
            )
            .expect("advance appended file mtime");
        let states = load_claude_sync_states(&db)?;
        let file_mtime = metadata_modified_nanos(&fs::metadata(&path).expect("stat appended file"));
        assert_eq!(
            sync_single_file(
                &db,
                &path,
                file_mtime,
                &states,
                &mut pricing_cache,
                None,
                &HashMap::new(),
            )?,
            (1, 0)
        );

        let conn = lock_conn!(db.conn);
        let imported: Vec<String> = conn
            .prepare(
                "SELECT request_id FROM proxy_request_logs
                 WHERE request_id LIKE 'session:legacy-tail-%'
                 ORDER BY request_id",
            )?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        assert_eq!(imported, vec!["session:legacy-tail-c"]);
        Ok(())
    }

    #[test]
    fn shared_v18_same_mtime_truncation_pins_cursor() -> Result<(), AppError> {
        let db = Database::memory()?;
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = r#"{"type":"assistant","message":{"id":"truncated-a","model":"claude-x","usage":{"input_tokens":10,"output_tokens":100},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#;
        let second = r#"{"type":"assistant","message":{"id":"truncated-b","model":"claude-x","usage":{"input_tokens":11,"output_tokens":200},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:01Z","sessionId":"s1"}"#;
        let original = format!("{first}\n{second}\n");
        let path = write_temp_jsonl(tmp.path(), "truncated-v18.jsonl", &original);
        let path_str = path.to_string_lossy().to_string();
        let metadata = fs::metadata(&path).expect("stat original file");
        let modified = metadata.modified().expect("read original mtime");
        let modified_nanos = metadata_modified_nanos(&metadata);
        let original_size = metadata.len();
        let (fingerprint, hint) = {
            let mut file = File::open(&path).expect("open original file");
            let fingerprint = shared_tail_fingerprint_from_file(&mut file, original_size)
                .expect("fingerprint original");
            let hint = resume_hint_from_shared_cursor(
                &mut file,
                &path_str,
                modified_nanos,
                0,
                original_size,
                serde_json::to_string(&ClaudeResumeState { session_id: None })
                    .expect("serialize state"),
            )
            .expect("build original hint");
            (fingerprint, hint)
        };

        fs::write(&path, format!("{first}\n")).expect("truncate file");
        File::options()
            .write(true)
            .open(&path)
            .expect("open truncated file")
            .set_times(fs::FileTimes::new().set_modified(modified))
            .expect("restore original mtime");

        let states = HashMap::from([(
            path_str.clone(),
            ClaudeSyncState {
                last_modified: modified_nanos,
                last_line_offset: 0,
                last_byte_offset: Some(original_size as i64),
                last_tail_fingerprint: Some(fingerprint),
            },
        )]);
        let hints = HashMap::from([(path_str.clone(), hint)]);
        let mut pricing_cache = PricingCache::new();
        let error = sync_single_file(
            &db,
            &path,
            modified_nanos,
            &states,
            &mut pricing_cache,
            None,
            &hints,
        )
        .expect_err("same-mtime truncation must be pinned");
        assert!(error.to_string().contains("截断"));

        let conn = lock_conn!(db.conn);
        let pinned: i64 = conn.query_row(
            "SELECT last_byte_offset FROM session_log_sync WHERE file_path = ?1",
            [&path_str],
            |row| row.get(0),
        )?;
        assert_eq!(
            pinned,
            fs::metadata(&path).expect("stat truncated file").len() as i64
        );
        Ok(())
    }

    #[test]
    fn shared_v18_rewrite_pins_cursor_without_replay() -> Result<(), AppError> {
        let db = Database::memory()?;
        let tmp = tempfile::tempdir().expect("tempdir");
        let original = r#"{"type":"assistant","message":{"id":"shared-a","model":"claude-x","usage":{"input_tokens":10,"output_tokens":100},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#;
        let replacement = original.replace("shared-a", "shared-x");
        assert_eq!(original.len(), replacement.len());
        let original_content = format!("{original}\n");
        let path = write_temp_jsonl(tmp.path(), "rewritten-v18.jsonl", &original_content);
        let path_str = path.to_string_lossy().to_string();
        let byte_offset = original_content.len() as u64;
        let original_fingerprint = {
            let mut file = File::open(&path).expect("open original cursor file");
            shared_tail_fingerprint_from_file(&mut file, byte_offset).expect("fingerprint original")
        };
        fs::write(&path, format!("{replacement}\n")).expect("rewrite shared cursor file");

        let mut states = HashMap::new();
        states.insert(
            path_str.clone(),
            ClaudeSyncState {
                last_modified: 1,
                last_line_offset: 0,
                last_byte_offset: Some(byte_offset as i64),
                last_tail_fingerprint: Some(original_fingerprint),
            },
        );
        // The local hint describes the rewritten bytes and would pass its own
        // 64-byte check. The shared 4 KiB fingerprint must still take priority.
        let matching_sidecar = {
            let mut file = File::open(&path).expect("open rewritten cursor file");
            resume_hint_from_shared_cursor(
                &mut file,
                &path_str,
                1,
                0,
                byte_offset,
                serde_json::to_string(&ClaudeResumeState { session_id: None })
                    .expect("serialize state"),
            )
            .expect("build matching local hint")
        };
        let hints = HashMap::from([(path_str.clone(), matching_sidecar)]);
        let mut pricing_cache = PricingCache::new();
        let error = sync_single_file(&db, &path, 2, &states, &mut pricing_cache, None, &hints)
            .expect_err("rewritten shared cursor must be pinned");
        assert!(
            error.to_string().contains("重写"),
            "unexpected error: {error}"
        );

        let conn = lock_conn!(db.conn);
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
            row.get(0)
        })?;
        assert_eq!(rows, 0, "rewritten history must not be replayed");
        let pinned: i64 = conn.query_row(
            "SELECT last_byte_offset FROM session_log_sync WHERE file_path = ?1",
            [&path_str],
            |row| row.get(0),
        )?;
        assert_eq!(
            pinned,
            fs::metadata(&path)
                .expect("stat rewritten cursor file")
                .len() as i64
        );
        Ok(())
    }

    /// 在临时目录写入一个 JSONL 文件并返回其路径。
    fn write_temp_jsonl(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).expect("write jsonl");
        path
    }

    fn test_claude_line(message_id: &str, output_tokens: u32) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"id":"{message_id}","model":"claude-x","usage":{{"input_tokens":10,"output_tokens":{output_tokens}}},"stop_reason":"end_turn"}},"timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}}"#
        )
    }

    /// 单文件多消息经由单事务写入后，imported/skipped 计数应与旧逐行自动提交
    /// 语义一致：只有 stop_reason 且 output_tokens>0 的条目参与插入；第二轮重扫
    /// 全部命中 request_id 去重、计为 skipped。
    #[test]
    fn test_sync_single_file_batch_counts() -> Result<(), AppError> {
        let db = Database::memory()?;
        let tmp = tempfile::tempdir().expect("tempdir");
        // m1/m2：完整条目应导入；m3：无 stop_reason 被过滤；m4：output=0 被过滤。
        let content = concat!(
            r#"{"type":"assistant","message":{"id":"m1","model":"claude-x","usage":{"input_tokens":10,"output_tokens":100,"cache_read_input_tokens":5,"cache_creation_input_tokens":3},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:00Z","sessionId":"s1"}"#,
            "\n",
            r#"{"type":"assistant","message":{"id":"m2","model":"claude-x","usage":{"input_tokens":11,"output_tokens":200},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:01Z","sessionId":"s1"}"#,
            "\n",
            r#"{"type":"assistant","message":{"id":"m3","model":"claude-x","usage":{"input_tokens":9,"output_tokens":50}},"timestamp":"2026-01-01T00:00:02Z","sessionId":"s1"}"#,
            "\n",
            r#"{"type":"assistant","message":{"id":"m4","model":"claude-x","usage":{"input_tokens":9,"output_tokens":0},"stop_reason":"end_turn"},"timestamp":"2026-01-01T00:00:03Z","sessionId":"s1"}"#,
            "\n",
        );
        let path = write_temp_jsonl(tmp.path(), "session.jsonl", content);

        let states: HashMap<String, ClaudeSyncState> = HashMap::new();
        let mut cache = PricingCache::new();

        // 首轮：m1/m2 导入，m3/m4 在插入前被过滤（既不计 imported 也不计 skipped）。
        let (imported, skipped) =
            sync_single_file(&db, &path, 1, &states, &mut cache, None, &HashMap::new())?;
        assert_eq!((imported, skipped), (2, 0));

        {
            let conn = lock_conn!(db.conn);
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
                    row.get(0)
                })?;
            assert_eq!(count, 2);
        }

        // 次轮：states 仍为空 → 重新解析，m1/m2 因 request_id 已存在被去重记为 skipped。
        let (imported2, skipped2) =
            sync_single_file(&db, &path, 1, &states, &mut cache, None, &HashMap::new())?;
        assert_eq!((imported2, skipped2), (0, 2));

        {
            let conn = lock_conn!(db.conn);
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
                    row.get(0)
                })?;
            assert_eq!(count, 2);
        }

        Ok(())
    }

    /// 预过滤 + 窄结构体解析与旧 Value 解析等价：
    /// - 首行为非 assistant 但携带 sessionId，需被解析以确定 session id；
    /// - 一条不含 "assistant" 子串的超大 user 行，session id 已知后应被跳过不解析；
    /// - assistant 行的紧凑与带空格两种写法都应被识别并正确抽取字段。
    #[test]
    fn test_prefilter_narrow_parse_parity() -> Result<(), AppError> {
        let db = Database::memory()?;
        let tmp = tempfile::tempdir().expect("tempdir");

        let big_blob = "x".repeat(200_000);
        assert!(!big_blob.contains("assistant"));
        let content = format!(
            concat!(
                r#"{{"type":"summary","sessionId":"sess-xyz"}}"#,
                "\n",
                r#"{{"type":"user","message":{{"role":"user","content":"{blob}"}}}}"#,
                "\n",
                r#"{{"type":"assistant","message":{{"id":"a1","model":"claude-x","usage":{{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":5,"cache_creation_input_tokens":3}},"stop_reason":"end_turn"}},"timestamp":"2026-01-01T00:00:00Z","sessionId":"sess-xyz"}}"#,
                "\n",
                r#"{{"type": "assistant", "message": {{"id": "a2", "model": "claude-x", "usage": {{"input_tokens": 11, "output_tokens": 21}}, "stop_reason": "end_turn"}}, "timestamp": "2026-01-01T00:00:01Z", "sessionId": "sess-xyz"}}"#,
                "\n",
            ),
            blob = big_blob
        );
        let path = write_temp_jsonl(tmp.path(), "session.jsonl", &content);

        let states: HashMap<String, ClaudeSyncState> = HashMap::new();
        let mut cache = PricingCache::new();
        let (imported, skipped) =
            sync_single_file(&db, &path, 1, &states, &mut cache, None, &HashMap::new())?;
        assert_eq!((imported, skipped), (2, 0));

        let conn = lock_conn!(db.conn);
        // a1：紧凑写法，四类 token 全带且 session id 来自首行的非 assistant 行。
        let (input1, output1, read1, creation1, sid1): (i64, i64, i64, i64, String) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, session_id
                 FROM proxy_request_logs WHERE request_id = 'session:a1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )?;
        assert_eq!((input1, output1, read1, creation1), (10, 20, 5, 3));
        assert_eq!(sid1, "sess-xyz");

        // a2：带空格写法，缺省的 cache 字段应回退为 0。
        let (input2, output2, read2, creation2, sid2): (i64, i64, i64, i64, String) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, session_id
                 FROM proxy_request_logs WHERE request_id = 'session:a2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )?;
        assert_eq!((input2, output2, read2, creation2), (11, 21, 0, 0));
        assert_eq!(sid2, "sess-xyz");

        Ok(())
    }

    /// 定价缓存命中时返回与直接查库完全一致的定价，据此计算的费用不变。
    #[test]
    fn test_cached_model_pricing_hit_matches_direct() -> Result<(), AppError> {
        let db = Database::memory()?;
        let conn = lock_conn!(db.conn);
        conn.execute(
            "INSERT OR REPLACE INTO model_pricing
                (model_id, display_name, input_cost_per_million, output_cost_per_million,
                 cache_read_cost_per_million, cache_creation_cost_per_million)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params!["test-cache-model", "Test", "3", "15", "0.3", "3.75"],
        )?;

        let direct = find_model_pricing(&conn, "test-cache-model").expect("direct pricing");

        let mut cache = PricingCache::new();
        let first = cached_model_pricing(&conn, &mut cache, "test-cache-model").expect("first");
        assert!(cache.contains_key("test-cache-model"));
        // 命中缓存的第二次调用不再查库，返回值应与首次及直接查库一致。
        let second = cached_model_pricing(&conn, &mut cache, "test-cache-model").expect("second");

        assert_eq!(direct.input_cost_per_million, first.input_cost_per_million);
        assert_eq!(
            direct.output_cost_per_million,
            first.output_cost_per_million
        );
        assert_eq!(
            direct.cache_read_cost_per_million,
            first.cache_read_cost_per_million
        );
        assert_eq!(
            direct.cache_creation_cost_per_million,
            first.cache_creation_cost_per_million
        );
        assert_eq!(first.input_cost_per_million, second.input_cost_per_million);
        assert_eq!(
            first.output_cost_per_million,
            second.output_cost_per_million
        );

        // 相同定价 → 相同费用。
        let usage = TokenUsage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 200,
            cache_creation_tokens: 100,
            model: Some("test-cache-model".to_string()),
            message_id: None,
        };
        let cost_direct = CostCalculator::calculate(&usage, &direct, Decimal::from(1));
        let cost_cached = CostCalculator::calculate(&usage, &second, Decimal::from(1));
        assert_eq!(cost_direct.total_cost, cost_cached.total_cost);

        Ok(())
    }
}
