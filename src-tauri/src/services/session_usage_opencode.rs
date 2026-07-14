//! OpenCode 会话日志使用追踪
//!
//! 从 ~/.local/share/opencode/opencode.db (SQLite) 中提取精确 token 使用数据。
//!
//! ## 数据流
//! ```text
//! ~/.local/share/opencode/opencode.db
//!   → session 表获取所有会话
//!   → message 表获取 assistant 消息
//!   → 解析 data JSON 提取 tokens/cost/model
//!   → proxy_request_logs 表
//! ```

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::opencode_config::get_opencode_db_path;
use crate::proxy::usage::calculator::CostCalculator;
use crate::proxy::usage::parser::TokenUsage;
use crate::services::session_usage::{
    cached_model_pricing, get_all_sync_states, metadata_modified_nanos, update_sync_state_conn,
    PricingCache, SessionSyncResult, SESSION_LOG_COMMIT_BATCH,
};
use crate::services::usage_stats::{should_skip_session_insert, DedupKey};
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::SystemTime;

/// 从 opencode message.data JSON 中提取的 token 和费用数据
struct OpenCodeMessageData {
    input_tokens: u32,
    output_tokens: u32,
    reasoning_tokens: u32,
    cache_read_tokens: u32,
    cache_write_tokens: u32,
    cost: f64,
    model_id: String,
    /// 入库时间戳（Unix 秒），在阶段一（解析/收集）就定死：`time.created` 缺失
    /// 或 <=0 时回退 now()。两阶段批量写库下不推迟到 insert 才取 now()，避免
    /// 退化输入（缺时间戳）的时间戳后移。
    created_at: i64,
}

/// 当前 Unix 时间（秒）。缺失/非法时间戳时的回退来源。
fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

struct OpenCodeMessageQueryResult {
    messages: Vec<(String, OpenCodeMessageData)>,
    has_incomplete_usage: bool,
}

/// 有界分批的批次元素：一条待写消息，或一个会话完成标记。
///
/// 与旧的"一个批次元素持有整会话全部消息"不同，这里把消息按 chunk 平铺入批：
/// 单个含数千消息的会话也会被切成多个 <= SESSION_LOG_COMMIT_BATCH 的 chunk，
/// 分多个短事务写入，不再形成超大事务。会话的**全部**消息入队完毕后追加一个
/// `SessionFinalize` 标记，真正的会话级状态推进发生在该标记被 flush 时（且仅当
/// 该会话所有 chunk 都无插入错误 + advance_state）。
enum PendingOpenCodeItem {
    /// 一条待写消息（携带所属会话，供跨 flush 的会话级错误归集与状态键推进）。
    Message {
        session_id: String,
        request_id: String,
        msg_data: OpenCodeMessageData,
    },
    /// 会话的全部消息已入队：flush 到达此标记时按条件推进会话级同步状态。
    SessionFinalize {
        session_id: String,
        sync_key: String,
        time_updated: i64,
        /// 消息查询完整且无 incomplete usage 时才推进会话级同步状态。
        advance_state: bool,
    },
}

/// 把一个批次写入主库：一个短事务覆盖批内全部消息插入与到达的会话完成标记，
/// 提交后清空批次释放内存。
///
/// `session_errors` 跨 flush 存续：会话任一 chunk 出现插入错误就记入，其
/// `SessionFinalize`（可能要到后续 flush 才被处理）据此跳过状态推进——从而保证
/// "会话全部消息写入尝试完成、无插入错误"才 finalize，与旧的整会话事务语义一致。
/// chunk 之间崩溃由 request_id 去重兜底：未 finalize 的会话下次整会话重扫，已写入
/// 的消息命中 request_id 去重。
fn flush_opencode_batch(
    db: &Database,
    pricing_cache: &mut PricingCache,
    batch: &mut Vec<PendingOpenCodeItem>,
    result: &mut SessionSyncResult,
    has_sync_errors: &mut bool,
    session_errors: &mut HashSet<String>,
) -> Result<(), AppError> {
    if batch.is_empty() {
        return Ok(());
    }

    let mut guard = lock_conn!(db.conn);
    let tx = guard
        .transaction()
        .map_err(|e| AppError::Database(format!("开启事务失败: {e}")))?;

    for item in batch.iter() {
        match item {
            PendingOpenCodeItem::Message {
                session_id,
                request_id,
                msg_data,
            } => {
                match insert_opencode_message(&tx, pricing_cache, request_id, msg_data, session_id)
                {
                    Ok(true) => result.imported += 1,
                    Ok(false) => result.skipped += 1,
                    Err(e) => {
                        let msg = format!("OpenCode 消息插入失败 {request_id}: {e}");
                        log::warn!("[OPENCODE-SYNC] {msg}");
                        result.errors.push(msg);
                        result.skipped += 1;
                        // 记入会话级错误：该会话的 finalize 将跳过状态推进（下次重试）。
                        session_errors.insert(session_id.clone());
                        *has_sync_errors = true;
                    }
                }
            }
            PendingOpenCodeItem::SessionFinalize {
                session_id,
                sync_key,
                time_updated,
                advance_state,
            } => {
                // 该会话任一 chunk 出现插入错误 → 不推进会话级状态（下次整会话重试）。
                if session_errors.remove(session_id) {
                    continue;
                }
                if !*advance_state {
                    continue;
                }
                // 更新会话级同步状态。失败时不要推进文件级状态，确保下次可重试。
                if let Err(e) = update_sync_state_conn(&tx, sync_key, *time_updated, 0) {
                    let msg = format!("OpenCode 会话同步状态更新失败 {sync_key}: {e}");
                    log::warn!("[OPENCODE-SYNC] {msg}");
                    result.errors.push(msg);
                    *has_sync_errors = true;
                }
            }
        }
    }

    tx.commit()
        .map_err(|e| AppError::Database(format!("提交事务失败: {e}")))?;
    drop(guard);

    batch.clear();
    Ok(())
}

/// 同步 OpenCode 使用数据
pub fn sync_opencode_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let db_path = get_opencode_db_path();

    if !db_path.exists() {
        return Ok(SessionSyncResult {
            imported: 0,
            skipped: 0,
            files_scanned: 0,
            errors: vec![],
        });
    }

    // OpenCode 是单个外部数据库，进度上按 1 个数据源计；guard 确保
    // 提前 return（未变化跳过/出错）也会计入 done。
    crate::services::session_usage::sync_progress::add_total(1);
    struct ProgressDoneGuard;
    impl Drop for ProgressDoneGuard {
        fn drop(&mut self) {
            crate::services::session_usage::sync_progress::add_done(1);
        }
    }
    let _done_on_exit = ProgressDoneGuard;

    let db_path_str = db_path.to_string_lossy().to_string();

    // 检查文件修改时间。
    // opencode 的数据库运行在 WAL 模式：新提交先落在 -wal 文件里，
    // 主库文件只有在 checkpoint 时才更新。因此必须同时考虑 -wal 的
    // mtime，否则会在 checkpoint 之前漏掉刚写入的会话。
    let metadata = fs::metadata(&db_path)
        .map_err(|e| AppError::Config(format!("无法读取 opencode.db 元数据: {e}")))?;
    let mut file_modified = metadata_modified_nanos(&metadata);

    let wal_path = db_path.with_extension("db-wal");
    if let Ok(wal_meta) = fs::metadata(&wal_path) {
        file_modified = file_modified.max(metadata_modified_nanos(&wal_meta));
    }

    // 一次性预载全部同步状态（文件级 + 会话级），避免逐会话查库；也让下方所有
    // 写入能在同一个写事务内完成，而不会与逐次 get_sync_state 的加锁互相自锁。
    let sync_states = get_all_sync_states(db)?;
    let (last_modified, _last_offset) = sync_states.get(&db_path_str).copied().unwrap_or((0, 0));

    // 文件未变化则跳过
    if file_modified <= last_modified {
        return Ok(SessionSyncResult {
            imported: 0,
            skipped: 0,
            files_scanned: 1,
            errors: vec![],
        });
    }

    // 打开 opencode 的 SQLite 数据库（只读）
    let opencode_conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| AppError::Database(format!("无法打开 opencode.db: {e}")))?;

    // 本次同步周期共享的定价缓存，避免每条消息重复查 model_pricing 表。
    let mut pricing_cache = PricingCache::new();

    sync_opencode_sessions_from_conn(
        db,
        &opencode_conn,
        &db_path_str,
        file_modified,
        &sync_states,
        &mut pricing_cache,
    )
}

/// 有界分批的核心：逐会话读外部 opencode 连接（期间不持主库锁），把每个会话的
/// 消息按 `SESSION_LOG_COMMIT_BATCH` 切成 chunk 平铺入批，凑满阈值就作为一个批次
/// 写入主库短事务并释放内存——单个含数千消息的巨会话也按 chunk 切齐，不再形成
/// 超大事务。会话的全部消息入队完毕后追加 `SessionFinalize` 标记，会话级状态推进
/// （无插入错误 + advance_state）发生在该标记被 flush 时；本轮完全成功才推进
/// 文件级状态。
///
/// 崩溃语义：批次（chunk）之间崩溃由 request_id 去重兜底——未 finalize 的会话
/// 下次整会话重扫，已写入的消息命中 request_id 去重。
///
/// 与 `sync_opencode_usage` 分离，便于用内存源库直接对核心分批逻辑做单元测试
/// （不触发路径解析与全局进度埋点）。
fn sync_opencode_sessions_from_conn(
    db: &Database,
    opencode_conn: &rusqlite::Connection,
    db_path_str: &str,
    file_modified: i64,
    sync_states: &HashMap<String, (i64, i64)>,
    pricing_cache: &mut PricingCache,
) -> Result<SessionSyncResult, AppError> {
    let mut result = SessionSyncResult {
        imported: 0,
        skipped: 0,
        files_scanned: 1,
        errors: vec![],
    };
    let mut has_sync_errors = false;

    // 查询所有会话
    let sessions = query_sessions(opencode_conn)?;

    let mut batch: Vec<PendingOpenCodeItem> = Vec::new();
    let mut batch_messages: usize = 0;
    // 跨 flush 存续的会话级插入错误集合：会话任一 chunk 出错即记入，其 finalize
    // 据此跳过状态推进（下次整会话重试）。
    let mut session_errors: HashSet<String> = HashSet::new();
    // fix 4：本轮是否见到任何 has_incomplete_usage 的会话。incomplete 消息不推进
    // 会话级 state（advance_state=false），但文件级 state 原先只要 !has_sync_errors
    // 就推进（incomplete 不算 error）。若某 incomplete 消息随后完成、而 db/wal
    // mtime 未超过已记录的文件级 mtime（同 tick / 低精度 fs），入口的
    // `file_modified <= last_modified` 会跳过整库、完成的消息漏导。故本轮存在任何
    // incomplete 时不推进文件级 state，强制下轮复查整库（未变会话仍由会话级 sync
    // state 跳过，重扫成本主要是 query_sessions 的 watermark 查询，可接受）。
    let mut saw_incomplete = false;

    for (session_id, time_updated) in &sessions {
        // 检查会话是否需要重新同步（从预载快照读取）
        let sync_key = format!("{db_path_str}:{session_id}");
        let (sess_last_modified, _) = sync_states.get(&sync_key).copied().unwrap_or((0, 0));
        if *time_updated <= sess_last_modified {
            continue; // 会话未更新，跳过
        }

        match query_assistant_messages(opencode_conn, session_id) {
            Ok(query_result) => {
                saw_incomplete |= query_result.has_incomplete_usage;
                let advance_state = !query_result.has_incomplete_usage;
                // 逐消息平铺入批；每凑满 SESSION_LOG_COMMIT_BATCH 条即 flush 一个短
                // 事务，单会话消息量再大也按 chunk 切齐，主库写锁窗口只覆盖单个批次。
                for (message_id, msg_data) in query_result.messages {
                    batch.push(PendingOpenCodeItem::Message {
                        session_id: session_id.clone(),
                        request_id: format!("opencode_session:{session_id}:{message_id}"),
                        msg_data,
                    });
                    batch_messages += 1;
                    if batch_messages >= SESSION_LOG_COMMIT_BATCH as usize {
                        flush_opencode_batch(
                            db,
                            pricing_cache,
                            &mut batch,
                            &mut result,
                            &mut has_sync_errors,
                            &mut session_errors,
                        )?;
                        batch_messages = 0;
                    }
                }
                // 会话全部消息已入队：追加 finalize 标记（不计入消息阈值，等下一次
                // 消息凑满阈值或末尾统一 flush 时随批处理）。
                batch.push(PendingOpenCodeItem::SessionFinalize {
                    session_id: session_id.clone(),
                    sync_key,
                    time_updated: *time_updated,
                    advance_state,
                });
            }
            Err(e) => {
                let msg = format!("OpenCode 会话消息查询失败 {session_id}: {e}");
                log::warn!("[OPENCODE-SYNC] {msg}");
                result.errors.push(msg);
                has_sync_errors = true;
            }
        }
    }

    // 冲刷剩余批次（末尾消息 chunk + 尚未处理的 finalize 标记）。
    flush_opencode_batch(
        db,
        pricing_cache,
        &mut batch,
        &mut result,
        &mut has_sync_errors,
        &mut session_errors,
    )?;

    // 仅在本轮完全成功且无 incomplete 会话时推进文件级状态；否则保留下次重试
    // 入口（fix 4：incomplete 会话可能在同一 mtime tick 内完成，不推进文件级
    // mtime 才能保证下轮不会 mtime-skip 整库而漏掉补全的消息）。
    if !has_sync_errors && !saw_incomplete {
        let mut guard = lock_conn!(db.conn);
        let tx = guard
            .transaction()
            .map_err(|e| AppError::Database(format!("开启事务失败: {e}")))?;
        update_sync_state_conn(&tx, db_path_str, file_modified, 0)?;
        tx.commit()
            .map_err(|e| AppError::Database(format!("提交事务失败: {e}")))?;
    }

    if result.imported > 0 {
        log::info!(
            "[OPENCODE-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, 扫描 {} 个会话",
            result.imported,
            result.skipped,
            sessions.len()
        );
    }

    Ok(result)
}

/// 查询所有会话的 (id, sync_watermark)
fn query_sessions(conn: &rusqlite::Connection) -> Result<Vec<(String, i64)>, AppError> {
    // ORDER BY ... DESC：最新会话最先入库。会话之间无状态依赖，处理顺序自由，
    // 取降序让 Usage 默认视图（Today/7d）在首次导入时尽快出数。
    let mut stmt = conn
        .prepare(
            "SELECT s.id,
                    MAX(s.time_updated, COALESCE(MAX(m.time_updated), s.time_updated)) AS sync_watermark
             FROM session s
             LEFT JOIN message m ON m.session_id = s.id
             GROUP BY s.id
             ORDER BY sync_watermark DESC",
        )
        .map_err(|e| AppError::Database(format!("准备会话查询失败: {e}")))?;

    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| AppError::Database(format!("查询会话失败: {e}")))?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(|e| AppError::Database(format!("读取会话行失败: {e}")))?);
    }

    Ok(sessions)
}

/// 查询某会话的已完成 assistant 消息，并标记是否还有未完成 usage 消息。
fn query_assistant_messages(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<OpenCodeMessageQueryResult, AppError> {
    let mut stmt = conn
        .prepare("SELECT id, data FROM message WHERE session_id = ?1 ORDER BY time_created")
        .map_err(|e| AppError::Database(format!("准备消息查询失败: {e}")))?;

    let rows = stmt
        .query_map([session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| AppError::Database(format!("查询消息失败: {e}")))?;

    let mut messages = Vec::new();
    let mut has_incomplete_usage = false;
    for row in rows {
        let (message_id, data_json) =
            row.map_err(|e| AppError::Database(format!("读取消息行失败: {e}")))?;

        // 只处理 assistant 消息
        let value: serde_json::Value = match serde_json::from_str(&data_json) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if value.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }

        // 必须有 tokens 字段
        if value.get("tokens").is_none() {
            continue;
        }

        // 跳过未完成的消息：进行中只有半截 token，且因 INSERT OR IGNORE 无法回填
        if value.get("time").and_then(|t| t.get("completed")).is_none() {
            has_incomplete_usage = true;
            continue;
        }

        if let Some(msg_data) = parse_message_data(&value) {
            messages.push((message_id, msg_data));
        }
    }

    Ok(OpenCodeMessageQueryResult {
        messages,
        has_incomplete_usage,
    })
}

/// 解析 opencode message.data JSON 为结构化数据
fn parse_message_data(value: &serde_json::Value) -> Option<OpenCodeMessageData> {
    let tokens = value.get("tokens")?;

    let input_tokens = tokens.get("input").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let output_tokens = tokens.get("output").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let reasoning_tokens = tokens
        .get("reasoning")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let cache_obj = tokens.get("cache");
    let cache_read_tokens = cache_obj
        .and_then(|c| c.get("read"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let cache_write_tokens = cache_obj
        .and_then(|c| c.get("write"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    // 跳过全零 token 的消息
    if input_tokens == 0
        && output_tokens == 0
        && reasoning_tokens == 0
        && cache_read_tokens == 0
        && cache_write_tokens == 0
    {
        return None;
    }

    let cost = value.get("cost").and_then(|v| v.as_f64()).unwrap_or(0.0);

    let model_id = value
        .get("modelID")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let timestamp_ms = value
        .get("time")
        .and_then(|t| t.get("created"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    // 在阶段一（解析/收集）就定死 created_at（秒）：缺失/<=0 回退 now()。
    let created_at = if timestamp_ms > 0 {
        timestamp_ms / 1000
    } else {
        now_unix_secs()
    };

    Some(OpenCodeMessageData {
        input_tokens,
        output_tokens,
        reasoning_tokens,
        cache_read_tokens,
        cache_write_tokens,
        cost,
        model_id,
        created_at,
    })
}

/// 插入单条 OpenCode 消息记录到 proxy_request_logs
///
/// 调用方在同一事务连接上批量调用本函数；INSERT 与去重查询走 prepare_cached，
/// 费用查询走 per-cycle 定价缓存。
fn insert_opencode_message(
    conn: &rusqlite::Connection,
    pricing_cache: &mut PricingCache,
    request_id: &str,
    msg: &OpenCodeMessageData,
    session_id: &str,
) -> Result<bool, AppError> {
    // created_at 由 parse_message_data 在阶段一定死（见其字段注释），insert 只消费。
    let created_at = msg.created_at;

    // OpenCode 使用 Anthropic 风格：input 是新鲜输入，cache 单独计
    // output 包含 reasoning tokens（按输出计费）
    let output_with_reasoning = msg.output_tokens + msg.reasoning_tokens;

    let dedup_key = DedupKey {
        app_type: "opencode",
        model: &msg.model_id,
        input_tokens: msg.input_tokens,
        output_tokens: output_with_reasoning,
        cache_read_tokens: msg.cache_read_tokens,
        cache_creation_tokens: msg.cache_write_tokens,
        created_at,
    };
    if should_skip_session_insert(conn, request_id, &dedup_key)? {
        return Ok(false);
    }

    // 如果 opencode 已经提供了费用，直接使用；否则从模型定价计算
    let (input_cost, output_cost, cache_read_cost, cache_creation_cost, total_cost) =
        if msg.cost > 0.0 {
            // opencode 已计算费用，直接使用
            // 简化处理：全部放入 total_cost（opencode 的 cost 是聚合值，无法精确拆分）
            (
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                msg.cost.to_string(),
            )
        } else {
            // opencode 费用为 0（如免费模型），尝试用 cc-switch 自带的模型定价计算
            let usage = TokenUsage {
                input_tokens: msg.input_tokens,
                output_tokens: output_with_reasoning,
                cache_read_tokens: msg.cache_read_tokens,
                cache_creation_tokens: msg.cache_write_tokens,
                model: Some(msg.model_id.clone()),
                message_id: None,
            };

            match cached_model_pricing(conn, pricing_cache, &msg.model_id) {
                Some(pricing) => {
                    let cost = CostCalculator::calculate_for_app(
                        "opencode",
                        &usage,
                        &pricing,
                        Decimal::from(1),
                    );
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
            }
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
        .map_err(|e| AppError::Database(format!("插入 OpenCode 会话日志失败: {e}")))?;
    let inserted_rows = stmt
        .execute(rusqlite::params![
            request_id,
            "_opencode_session", // provider_id
            "opencode",          // app_type
            msg.model_id,
            msg.model_id, // request_model = model
            msg.input_tokens,
            output_with_reasoning,
            msg.cache_read_tokens,
            msg.cache_write_tokens,
            input_cost,
            output_cost,
            cache_read_cost,
            cache_creation_cost,
            total_cost,
            0i64,                   // latency_ms
            Option::<i64>::None,    // first_token_ms
            200i64,                 // status_code
            Option::<String>::None, // error_message
            Some(session_id.to_string()),
            Some("opencode_session"), // provider_type
            1i64,                     // is_streaming
            "1.0",                    // cost_multiplier
            created_at,
            "opencode_session", // data_source
        ])
        .map_err(|e| AppError::Database(format!("插入 OpenCode 会话日志失败: {e}")))?;

    Ok(inserted_rows > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_message_data_full() {
        let json: serde_json::Value = serde_json::json!({
            "role": "assistant",
            "cost": 0.0023113,
            "tokens": {
                "total": 56554,
                "input": 3272,
                "output": 383,
                "reasoning": 419,
                "cache": {
                    "write": 0,
                    "read": 52480
                }
            },
            "modelID": "deepseek-v4-pro",
            "providerID": "deepseek",
            "time": {
                "created": 1779755333700i64,
                "completed": 1779755350639i64
            }
        });
        let data = parse_message_data(&json).unwrap();
        assert_eq!(data.input_tokens, 3272);
        assert_eq!(data.output_tokens, 383);
        assert_eq!(data.reasoning_tokens, 419);
        assert_eq!(data.cache_read_tokens, 52480);
        assert_eq!(data.cache_write_tokens, 0);
        assert!((data.cost - 0.0023113).abs() < 1e-10);
        assert_eq!(data.model_id, "deepseek-v4-pro");
        // created_at 在解析阶段定死：time.created(ms) / 1000
        assert_eq!(data.created_at, 1_779_755_333);
    }

    #[test]
    fn test_parse_message_data_missing_timestamp_falls_back_to_now() {
        // 无 time.created → created_at 在解析阶段回退 now()，落在解析前后窗口内。
        let before = now_unix_secs();
        let json: serde_json::Value = serde_json::json!({
            "role": "assistant",
            "tokens": { "input": 10, "output": 5 },
            "modelID": "m"
        });
        let data = parse_message_data(&json).unwrap();
        let after = now_unix_secs();
        assert!(before <= data.created_at && data.created_at <= after);
    }

    #[test]
    fn test_parse_message_data_missing_cache() {
        let json: serde_json::Value = serde_json::json!({
            "role": "assistant",
            "cost": 0.0,
            "tokens": {
                "input": 1000,
                "output": 200
            },
            "modelID": "mimo-v2.5-pro",
            "time": { "created": 1779755333700i64 }
        });
        let data = parse_message_data(&json).unwrap();
        assert_eq!(data.input_tokens, 1000);
        assert_eq!(data.output_tokens, 200);
        assert_eq!(data.reasoning_tokens, 0);
        assert_eq!(data.cache_read_tokens, 0);
        assert_eq!(data.cache_write_tokens, 0);
    }

    #[test]
    fn test_parse_message_data_skips_zero_tokens() {
        let json: serde_json::Value = serde_json::json!({
            "role": "assistant",
            "tokens": {
                "input": 0,
                "output": 0,
                "reasoning": 0,
                "cache": { "read": 0, "write": 0 }
            },
            "modelID": "test"
        });
        assert!(parse_message_data(&json).is_none());
    }

    #[test]
    fn test_parse_message_data_ignores_role() {
        // parse_message_data does not filter by role; that's the caller's job
        let json: serde_json::Value = serde_json::json!({
            "role": "user",
            "tokens": { "input": 100, "output": 0 }
        });
        let data = parse_message_data(&json).unwrap();
        assert_eq!(data.input_tokens, 100);
    }

    #[test]
    fn test_query_assistant_messages_skips_incomplete() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();

        let done = serde_json::json!({
            "role": "assistant",
            "tokens": { "input": 1000, "output": 200 },
            "modelID": "m",
            "time": { "created": 1, "completed": 2 }
        })
        .to_string();
        let in_progress = serde_json::json!({
            "role": "assistant",
            "tokens": { "input": 500, "output": 0 },
            "modelID": "m",
            "time": { "created": 3 }
        })
        .to_string();

        conn.execute(
            "INSERT INTO message VALUES ('done', 's1', 1, ?1), ('wip', 's1', 2, ?2)",
            rusqlite::params![done, in_progress],
        )
        .unwrap();

        let result = query_assistant_messages(&conn, "s1").unwrap();
        // 只返回已完成（带 time.completed）的消息，半截的被跳过
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].0, "done");
        assert!(result.has_incomplete_usage);
    }

    #[test]
    fn test_query_sessions_uses_message_update_watermark() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT, time_updated INTEGER);
             CREATE TABLE message (
                 id TEXT,
                 session_id TEXT,
                 time_created INTEGER,
                 time_updated INTEGER,
                 data TEXT
             );
             INSERT INTO session VALUES ('s1', 100);
             INSERT INTO message VALUES ('m1', 's1', 90, 200, '{}');",
        )
        .unwrap();

        let sessions = query_sessions(&conn).unwrap();
        assert_eq!(sessions, vec![("s1".to_string(), 200)]);
    }

    /// query_sessions 按 sync_watermark 降序返回（最新会话最先入库）。
    #[test]
    fn test_query_sessions_orders_newest_first() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT, time_updated INTEGER);
             CREATE TABLE message (
                 id TEXT,
                 session_id TEXT,
                 time_created INTEGER,
                 time_updated INTEGER,
                 data TEXT
             );
             INSERT INTO session VALUES ('old', 100), ('new', 300), ('mid', 200);",
        )
        .unwrap();

        let sessions = query_sessions(&conn).unwrap();
        // sync_watermark 降序：new(300) → mid(200) → old(100)
        assert_eq!(
            sessions,
            vec![
                ("new".to_string(), 300),
                ("mid".to_string(), 200),
                ("old".to_string(), 100),
            ]
        );
    }

    /// fix 1：单个会话消息数 > SESSION_LOG_COMMIT_BATCH 时，分批按 chunk 切齐
    /// （会话内部跨多个短事务写入，不再形成超大事务），断言全部消息导入、会话级
    /// 与文件级同步状态均已推进；第二轮（预载 state 后）会话 watermark 已达 → 会话
    /// 整体跳过，零导入零跳过。
    ///
    /// 注：单条消息插入失败的错误路径不易稳定构造，由现有 flush 逻辑
    /// （session_errors 记入 → finalize 跳过 + has_sync_errors 阻止文件级推进）
    /// 与 `should_skip_session_insert` 的 request_id 去重共同覆盖。
    #[test]
    fn test_sync_opencode_large_session_chunks_and_advances_state() -> Result<(), AppError> {
        let db = Database::memory()?;

        // 内存 opencode 源库：单会话含 > SESSION_LOG_COMMIT_BATCH 条已完成消息。
        let src = rusqlite::Connection::open_in_memory().unwrap();
        src.execute_batch(
            "CREATE TABLE session (id TEXT, time_updated INTEGER);
             CREATE TABLE message (
                 id TEXT,
                 session_id TEXT,
                 time_created INTEGER,
                 time_updated INTEGER,
                 data TEXT
             );
             INSERT INTO session VALUES ('s1', 100);",
        )
        .unwrap();

        // 501 > 阈值(500)：会话内部必然被切成 2 个 chunk、跨 2 个事务写入。
        let msg_count = SESSION_LOG_COMMIT_BATCH as usize + 1;
        {
            // time_updated 固定为 50（< session 的 100），使会话 watermark = 100 稳定可断言。
            let mut stmt = src
                .prepare("INSERT INTO message VALUES (?1, 's1', ?2, 50, ?3)")
                .unwrap();
            for i in 0..msg_count {
                // input tokens 唯一：即便走跨源指纹去重也不会互相误判（此处更是无关）。
                let data = serde_json::json!({
                    "role": "assistant",
                    "tokens": { "input": 10 + i as u64, "output": 5 },
                    "modelID": "m",
                    "time": { "created": 1000 + i as i64, "completed": 2000 + i as i64 }
                })
                .to_string();
                stmt.execute(rusqlite::params![format!("msg-{i}"), i as i64, data])
                    .unwrap();
            }
        }

        let db_path_str = "/tmp/opencode-batch-test.db";
        let empty_states: HashMap<String, (i64, i64)> = HashMap::new();
        let mut cache = PricingCache::new();

        let result = sync_opencode_sessions_from_conn(
            &db,
            &src,
            db_path_str,
            999,
            &empty_states,
            &mut cache,
        )?;
        assert_eq!(result.imported, msg_count as u32, "全部消息导入");
        assert_eq!(result.skipped, 0);
        assert!(result.errors.is_empty());

        // 全部消息落库
        {
            let conn = db.conn.lock().expect("lock conn");
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |r| r.get(0))?;
            assert_eq!(count, msg_count as i64);
        }

        // 会话级与文件级同步状态均已推进
        let states = get_all_sync_states(&db)?;
        assert_eq!(
            states.get(&format!("{db_path_str}:s1")).copied(),
            Some((100, 0)),
            "会话级 state 推进到 watermark=100"
        );
        assert_eq!(
            states.get(db_path_str).copied(),
            Some((999, 0)),
            "文件级 state 推进到 file_modified=999"
        );

        // 第二轮：预载 state 后会话 watermark 已达 → 会话被跳过，零导入零跳过。
        let result2 =
            sync_opencode_sessions_from_conn(&db, &src, db_path_str, 999, &states, &mut cache)?;
        assert_eq!((result2.imported, result2.skipped), (0, 0), "全 skip");

        Ok(())
    }

    /// fix 4：本轮存在 has_incomplete_usage 的会话时，文件级 state 不推进——否则
    /// incomplete 消息随后在同一 mtime tick 内完成，会因 `file_modified <=
    /// last_modified` 被整库 mtime-skip 而漏导。构造一条已完成 + 一条未完成
    /// （无 time.completed）的会话，断言文件级 state 未写入（下轮不会 mtime-skip
    /// 整库）；会话级 state 亦不推进（advance_state=false）。
    #[test]
    fn test_sync_opencode_incomplete_usage_does_not_advance_file_state() -> Result<(), AppError> {
        let db = Database::memory()?;
        let src = rusqlite::Connection::open_in_memory().unwrap();
        src.execute_batch(
            "CREATE TABLE session (id TEXT, time_updated INTEGER);
             CREATE TABLE message (
                 id TEXT,
                 session_id TEXT,
                 time_created INTEGER,
                 time_updated INTEGER,
                 data TEXT
             );
             INSERT INTO session VALUES ('s1', 100);",
        )
        .unwrap();

        // done：带 time.completed → 已完成，导入；wip：无 completed → incomplete。
        let done = serde_json::json!({
            "role": "assistant",
            "tokens": { "input": 10, "output": 5 },
            "modelID": "m",
            "time": { "created": 1, "completed": 2 }
        })
        .to_string();
        let wip = serde_json::json!({
            "role": "assistant",
            "tokens": { "input": 7, "output": 0 },
            "modelID": "m",
            "time": { "created": 3 }
        })
        .to_string();
        src.execute(
            "INSERT INTO message VALUES ('done','s1',1,50,?1),('wip','s1',2,50,?2)",
            rusqlite::params![done, wip],
        )
        .unwrap();

        let db_path_str = "/tmp/opencode-incomplete-test.db";
        let empty_states: HashMap<String, (i64, i64)> = HashMap::new();
        let mut cache = PricingCache::new();

        let result = sync_opencode_sessions_from_conn(
            &db,
            &src,
            db_path_str,
            999,
            &empty_states,
            &mut cache,
        )?;
        assert_eq!(result.imported, 1, "只导入已完成的消息");
        assert!(result.errors.is_empty());

        let states = get_all_sync_states(&db)?;
        assert_eq!(
            states.get(&format!("{db_path_str}:s1")).copied(),
            None,
            "incomplete 会话级 state 不推进（advance_state=false）"
        );
        assert_eq!(
            states.get(db_path_str).copied(),
            None,
            "存在 incomplete 时文件级 state 不推进 → 下轮不会 mtime-skip 整库"
        );

        Ok(())
    }
}
