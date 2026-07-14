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
//! - `session_meta` → 提取 session_id
//! - `turn_context` → 提取当前 model
//! - `event_msg` (type=token_count) → 提取累计 token 用量，计算 delta

use crate::codex_config::get_codex_config_dir;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::proxy::usage::calculator::CostCalculator;
use crate::proxy::usage::parser::TokenUsage;
use crate::services::session_usage::{
    cached_model_pricing, get_sync_state, metadata_modified_nanos, update_sync_state_conn,
    PricingCache, SessionSyncResult, SESSION_LOG_COMMIT_BATCH,
};
use crate::services::session_usage_driver::{save_resume_hint, scan_jsonl_incremental};
use crate::services::usage_stats::{should_skip_session_insert, DedupKey};
use crate::session_manager::scan_cache_store::{ScanCacheStore, SyncResumeHint};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 累计 token 用量（跟踪 total_token_usage 字段）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CumulativeTokens {
    input: u64,
    cached_input: u64,
    output: u64,
}

/// 单次 API 调用的 token 增量
#[derive(Debug)]
struct DeltaTokens {
    input: u32,
    cached_input: u32,
    output: u32,
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
    session_id: Option<String>,
    current_model: String,
    prev_total: Option<CumulativeTokens>,
    event_index: u32,
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

/// 同步 Codex 使用数据（从 JSONL 会话日志）
pub fn sync_codex_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let codex_dir = get_codex_config_dir();

    let files = collect_codex_session_files(&codex_dir);

    let mut result = SessionSyncResult {
        imported: 0,
        skipped: 0,
        files_scanned: files.len() as u32,
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
            &mut pricing_cache,
            resume_store.as_ref(),
            &resume_hints,
        ) {
            Ok((imported, skipped)) => {
                result.imported += imported;
                result.skipped += skipped;
            }
            Err(e) => {
                let msg = format!("Codex 会话文件解析失败 {}: {e}", file_path.display());
                log::warn!("[CODEX-SYNC] {msg}");
                result.errors.push(msg);
            }
        }
        crate::services::session_usage::sync_progress::add_done(1);
    }

    if result.imported > 0 {
        log::info!(
            "[CODEX-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, 扫描 {} 个文件",
            result.imported,
            result.skipped,
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
/// `file_mtime` 为 walk 阶段取得的 mtime 纳秒值；>0 时直接复用避免二次 stat，
/// 为 0 时回退到一次 metadata 读取，保留“元数据不可读即报错”语义。
///
/// `resume` 提供 sidecar 字节续传提示：Codex 的行跳过发生在解析之后（需要重放
/// 历史事件重建累计值状态），因此提示除字节位置外还必须携带可反序列化的
/// `FileParseState`；命中时 seek + 恢复状态机，彻底跳过历史行的重解析。
fn sync_single_codex_file(
    db: &Database,
    file_path: &Path,
    file_mtime: i64,
    pricing_cache: &mut PricingCache,
    resume: Option<&ScanCacheStore>,
    resume_hints: &HashMap<String, SyncResumeHint>,
) -> Result<(u32, u32), AppError> {
    let file_path_str = file_path.to_string_lossy().to_string();

    // 检查同步状态
    let (last_modified, last_offset) = get_sync_state(db, &file_path_str)?;

    // 扫描阶段：文件驱动归通用驱动，解析归下面的回调；先收集待写记录，
    // 写库阶段再统一批量落库（读文件期间不持有连接锁）。
    let mut pending: Vec<PendingCodexEntry> = Vec::new();

    // fix 2：续传提示取自预载 map（零 per-file 查询）；sidecar 是否可用另行传入。
    let hint = resume_hints.get(&file_path_str).cloned();

    let outcome = scan_jsonl_incremental(
        file_path,
        file_mtime,
        last_modified,
        last_offset,
        hint,
        resume.is_some(),
        || FileParseState {
            session_id: None,
            current_model: "unknown".to_string(),
            prev_total: None,
            event_index: 0,
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
                "session_meta" if state.session_id.is_none() => {
                    let payload = value.get("payload");
                    state.session_id = payload
                        .and_then(|p| {
                            p.get("session_id")
                                .or_else(|| p.get("sessionId"))
                                .or_else(|| p.get("id"))
                        })
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
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

                    if delta.is_zero() {
                        return; // 跳过 task 边界的零 delta 事件
                    }

                    state.event_index += 1;

                    // 历史行（仅无续传提示的回退路径）只重放重建状态，不产出记录
                    if !is_new {
                        return;
                    }

                    // 生成唯一 request_id
                    let session_id_str = state.session_id.as_deref().unwrap_or("unknown");
                    let request_id =
                        format!("codex_session:{}:{}", session_id_str, state.event_index);

                    // 在入队处（解析附近）就定死 created_at：缺失/非法 timestamp
                    // 回退 now()，避免两阶段写库时才取 now() 造成退化输入时间戳后移。
                    let created_at =
                        resolve_codex_created_at(value.get("timestamp").and_then(|v| v.as_str()));

                    pending.push(PendingCodexEntry {
                        request_id,
                        delta,
                        model: state.current_model.clone(),
                        session_id: state.session_id.clone(),
                        created_at,
                    });
                }
                _ => {}
            }
        },
    )?;

    // 文件未变化（mtime 跳过）
    let Some(outcome) = outcome else {
        return Ok((0, 0));
    };

    // 写库阶段：一个事务批量写入，超大文件每 SESSION_LOG_COMMIT_BATCH 行分段提交
    let mut imported: u32 = 0;
    let mut skipped: u32 = 0;

    let mut guard = lock_conn!(db.conn);
    let mut tx = guard
        .transaction()
        .map_err(|e| AppError::Database(format!("开启事务失败: {e}")))?;
    let mut since_commit: u32 = 0;

    for entry in &pending {
        match insert_codex_session_entry(
            &tx,
            pricing_cache,
            &entry.request_id,
            &entry.delta,
            &entry.model,
            entry.session_id.as_deref(),
            entry.created_at,
        ) {
            Ok(true) => imported += 1,
            Ok(false) => skipped += 1,
            Err(e) => {
                log::warn!("[CODEX-SYNC] 插入失败 ({}): {e}", entry.request_id);
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
    update_sync_state_conn(
        &tx,
        &file_path_str,
        outcome.file_modified,
        outcome.line_offset,
    )?;
    tx.commit()
        .map_err(|e| AppError::Database(format!("提交事务失败: {e}")))?;
    drop(guard);

    // 主库进度提交成功后，把字节位置与状态机写回 sidecar（尽力而为）
    save_resume_hint(resume, &file_path_str, &outcome);

    // 每个文件若有新插入行，只通知一次（旧实现为每行一次）
    if imported > 0 {
        crate::usage_events::notify_log_recorded();
    }

    Ok((imported, skipped))
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
    use crate::session_manager::scan_cache_store::ScanCacheStore;

    /// 状态机持久化判别测试：sync1 后把整个历史部分覆写成等长垃圾（丢失
    /// session_meta / turn_context / e1），再追加 e2。回退路径既无法重建
    /// prev_total 也会因行号变化误跳新行；字节续传路径从 sidecar 恢复完整
    /// 状态机，导入的 e2 必须是与 e1 的差值（150/30）而非累计值（250/80），
    /// request_id 的 event_index 也必须接着上次（:2）。
    #[test]
    fn test_codex_resume_restores_cumulative_state() -> Result<(), AppError> {
        let meta = r#"{"type":"session_meta","payload":{"session_id":"sess-1"}}"#;
        let turn = r#"{"type":"turn_context","payload":{"model":"gpt-5"}}"#;
        let e1 = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50}}}}"#;
        let e2 = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":250,"cached_input_tokens":0,"output_tokens":80}}}}"#;

        let db = crate::database::Database::memory()?;
        let store = ScanCacheStore::in_memory()?;
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("rollout.jsonl");
        let head = format!("{meta}\n{turn}\n{e1}\n");
        fs::write(&path, &head).expect("write");

        let mut cache = PricingCache::new();
        // fix 2：提示由调用方预载后传入；测试每次调用前从 store 现取（等价生产侧
        // 从预载 map 查找），使第二次调用能拿到第一次保存的续传提示。
        let hints = store.load_all_sync_resume().unwrap_or_default();
        let (imported, _) =
            sync_single_codex_file(&db, &path, 1, &mut cache, Some(&store), &hints)?;
        assert_eq!(imported, 1);

        // 把 session_meta/turn_context 两行覆写为等长垃圾（e1 行保持原样，
        // 尾部指纹窗口不受影响），再追加 e2。回退路径会因 meta 行损坏而把
        // session 记为 unknown、且需重放 e1 重建状态；续传路径直接从 sidecar
        // 恢复 sess-1/gpt-5/prev_total/event_index。
        let prefix_len = meta.len() + 1 + turn.len() + 1;
        let junk = "x".repeat(prefix_len - 1) + "\n";
        fs::write(&path, format!("{junk}{e1}\n{e2}\n")).expect("rewrite");

        let hints = store.load_all_sync_resume().unwrap_or_default();
        let (imported2, skipped2) =
            sync_single_codex_file(&db, &path, 2, &mut cache, Some(&store), &hints)?;
        assert_eq!((imported2, skipped2), (1, 0));

        let conn = lock_conn!(db.conn);
        let (input, output): (i64, i64) = conn.query_row(
            "SELECT input_tokens, output_tokens FROM proxy_request_logs
             WHERE request_id = 'codex_session:sess-1:2'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!((input, output), (150, 30));

        Ok(())
    }

    /// created_at 的确定性来源固化：token_count 行带显式 timestamp 时，入库
    /// created_at 必须等于解析出的时间戳（在扫描入队处定死），与写库时刻无关。
    /// 这是修复"两阶段延迟使退化输入时间戳后移"的可测形式——created_at 的来源是
    /// 解析出的时间戳（now() 无法 mock，故退化输入改由下方 helper 单测覆盖）。
    #[test]
    fn test_codex_created_at_from_parsed_timestamp() -> Result<(), AppError> {
        let meta = r#"{"type":"session_meta","payload":{"session_id":"sess-ts"}}"#;
        let turn = r#"{"type":"turn_context","payload":{"model":"gpt-5"}}"#;
        let event = r#"{"type":"event_msg","timestamp":"2026-01-02T03:04:05Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50}}}}"#;

        let db = Database::memory()?;
        let store = ScanCacheStore::in_memory()?;
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("rollout.jsonl");
        fs::write(&path, format!("{meta}\n{turn}\n{event}\n")).expect("write");

        let mut cache = PricingCache::new();
        let hints = store.load_all_sync_resume().unwrap_or_default();
        let (imported, _) =
            sync_single_codex_file(&db, &path, 1, &mut cache, Some(&store), &hints)?;
        assert_eq!(imported, 1);

        let expected = resolve_codex_created_at(Some("2026-01-02T03:04:05Z"));
        let conn = lock_conn!(db.conn);
        let created_at: i64 = conn.query_row(
            "SELECT created_at FROM proxy_request_logs WHERE request_id = 'codex_session:sess-ts:1'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(created_at, expected);
        Ok(())
    }

    /// 缺失/非法 timestamp 回退 now()，合法 rfc3339 走确定性解析。回退发生在入队
    /// 处（本 helper 被扫描回调调用），写库阶段不再取 now()。
    #[test]
    fn test_resolve_codex_created_at_fallback_and_parse() {
        // 合法 rfc3339 → 精确秒（00:16:45 = 1005s）
        assert_eq!(resolve_codex_created_at(Some("1970-01-01T00:16:45Z")), 1005);

        // 缺失 → 回退当前时刻，落在调用前后窗口内
        let before = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let got = resolve_codex_created_at(None);
        let after = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(before <= got && got <= after);

        // 非法字符串 → 同样回退 now()
        assert!(resolve_codex_created_at(Some("not-a-timestamp")) >= before);
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
        let mut pricing_cache = PricingCache::new();
        let inserted = {
            let conn = lock_conn!(db.conn);
            insert_codex_session_entry(
                &conn,
                &mut pricing_cache,
                "codex-session-dup",
                &delta,
                "gpt-5.4",
                Some("session-1"),
                resolve_codex_created_at(Some("1970-01-01T00:16:45Z")),
            )?
        };
        assert!(!inserted);

        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
            row.get(0)
        })?;
        assert_eq!(count, 1);

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
