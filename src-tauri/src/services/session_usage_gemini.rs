//! Gemini CLI 会话日志使用追踪
//!
//! 从 ~/.gemini/tmp/<project_hash>/chats/session-*.json 中提取精确 token 使用数据。
//!
//! ## 数据流
//! ```text
//! ~/.gemini/tmp/*/chats/session-*.json → 全量解析 → 费用计算 → proxy_request_logs 表
//! ```
//!
//! ## 与 Claude/Codex 解析器的差异
//! - JSON 格式（非 JSONL）：每个文件是单个 JSON 对象，包含 messages 数组
//! - 无需 delta 计算：tokens 字段是 per-message 独立值
//! - 无需状态恢复：不依赖前一条消息的累计值
//! - 天然去重：每条消息有唯一 id 字段

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::gemini_config::get_gemini_dir;
use crate::proxy::usage::calculator::CostCalculator;
use crate::proxy::usage::parser::TokenUsage;
use crate::services::session_usage::{
    cached_model_pricing, get_sync_state, metadata_modified_nanos, update_sync_state_conn,
    PricingCache, SessionSyncResult, SESSION_LOG_COMMIT_BATCH,
};
use crate::services::usage_stats::{has_matching_proxy_usage_log, DedupKey};
use rusqlite::OptionalExtension;
use rust_decimal::Decimal;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 从 Gemini message 中提取的 token 数据
#[derive(Debug)]
struct GeminiTokens {
    input: u32,
    output: u32,
    cached: u32,
    thoughts: u32,
}

/// 同步 Gemini 使用数据（从 JSON 会话日志）
pub fn sync_gemini_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let gemini_dir = get_gemini_dir();

    let files = collect_gemini_session_files(&gemini_dir);

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

    crate::services::session_usage::sync_progress::add_total(files.len() as u32);

    for (file_path, file_mtime) in &files {
        match sync_single_gemini_file(db, file_path, *file_mtime, &mut pricing_cache) {
            Ok((imported, skipped)) => {
                result.imported += imported;
                result.skipped += skipped;
            }
            Err(e) => {
                let msg = format!("Gemini 会话文件解析失败 {}: {e}", file_path.display());
                log::warn!("[GEMINI-SYNC] {msg}");
                result.errors.push(msg);
            }
        }
        crate::services::session_usage::sync_progress::add_done(1);
    }

    if result.imported > 0 {
        log::info!(
            "[GEMINI-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, 扫描 {} 个文件",
            result.imported,
            result.skipped,
            result.files_scanned
        );
    }

    Ok(result)
}

/// 收集所有 Gemini 会话 JSON 文件，返回 `(路径, mtime 纳秒)` 并按 mtime 降序
/// 排序（最近修改的会话最先入库）。walk 阶段顺带取 mtime，既用于排序也传给
/// 后续 `sync_single_gemini_file`，避免二次 stat（读取失败记 0，交由后者回退
/// 处理）。照抄 claude/codex 的 `push_xxx_file` 模式。
fn collect_gemini_session_files(gemini_dir: &Path) -> Vec<(PathBuf, i64)> {
    let mut files: Vec<(PathBuf, i64)> = Vec::new();

    let tmp_dir = gemini_dir.join("tmp");
    if !tmp_dir.is_dir() {
        return files;
    }

    // 遍历 tmp/<project_hash>/chats/session-*.json
    let project_dirs = match fs::read_dir(&tmp_dir) {
        Ok(entries) => entries,
        Err(_) => return files,
    };

    for entry in project_dirs.flatten() {
        let chats_dir = entry.path().join("chats");
        if !chats_dir.is_dir() {
            continue;
        }

        let chat_files = match fs::read_dir(&chats_dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for file_entry in chat_files.flatten() {
            let path = file_entry.path();
            let is_session = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("session-") && n.ends_with(".json"))
                .unwrap_or(false);
            if is_session {
                push_gemini_file(&mut files, path);
            }
        }
    }

    // mtime 降序：首次导入时最近的会话最先入库，Usage 默认 Today/7d 尽快出数。
    files.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    files
}

/// 记录一个 Gemini 会话文件及其 mtime（读取失败记 0）。
fn push_gemini_file(files: &mut Vec<(PathBuf, i64)>, path: PathBuf) {
    let mtime = fs::metadata(&path)
        .map(|m| metadata_modified_nanos(&m))
        .unwrap_or(0);
    files.push((path, mtime));
}

/// 同步单个 Gemini 会话 JSON 文件，返回 (imported, skipped)
///
/// `file_mtime` 为 walk 阶段取得的 mtime 纳秒值；>0 时直接复用避免二次 stat，
/// 为 0 时回退一次 metadata 读取，保留“元数据不可读即报错”语义。Gemini 是整
/// 文件解析、无字节续传，仅遵循 mtime 跳过契约；但对齐 JSONL 驱动，"将要跳过
/// 前"重取一次新鲜 mtime 复查，关闭 walk→处理之间文件被追加/重写的窗口。
fn sync_single_gemini_file(
    db: &Database,
    file_path: &Path,
    file_mtime: i64,
    pricing_cache: &mut PricingCache,
) -> Result<(u32, u32), AppError> {
    let file_path_str = file_path.to_string_lossy().to_string();

    // mtime：优先复用 walk 阶段的值，为 0 时回退一次 metadata 读取。
    let mut file_modified = if file_mtime > 0 {
        file_mtime
    } else {
        let metadata = fs::metadata(file_path)
            .map_err(|e| AppError::Config(format!("无法读取文件元数据: {e}")))?;
        metadata_modified_nanos(&metadata)
    };

    // 检查同步状态
    let (last_modified, _last_offset) = get_sync_state(db, &file_path_str)?;

    // walk 阶段的 mtime 可能在长同步期间过期：将要跳过前重取一次新鲜 mtime 复查，
    // 关闭 walk→处理之间文件被追加的窗口。重取失败（文件可能刚被删除）按原值跳过。
    if file_mtime > 0 && file_modified <= last_modified {
        if let Ok(metadata) = fs::metadata(file_path) {
            file_modified = metadata_modified_nanos(&metadata);
        }
    }

    // 文件未变化则跳过
    if file_modified <= last_modified {
        return Ok((0, 0));
    }

    // 读取并解析整个 JSON 文件
    let content = fs::read_to_string(file_path)
        .map_err(|e| AppError::Config(format!("无法读取文件: {e}")))?;
    let value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| AppError::Config(format!("JSON 解析失败: {e}")))?;

    // 提取顶层 sessionId
    let session_id = value
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // 遍历 messages 数组
    let messages = match value.get("messages").and_then(|v| v.as_array()) {
        Some(msgs) => msgs,
        None => return Ok((0, 0)),
    };

    let mut imported: u32 = 0;
    let mut skipped: u32 = 0;
    let mut gemini_msg_count: i64 = 0;

    // 整文件在一个事务内批量写入，超大文件每 SESSION_LOG_COMMIT_BATCH 行分段提交。
    let mut guard = lock_conn!(db.conn);
    let mut tx = guard
        .transaction()
        .map_err(|e| AppError::Database(format!("开启事务失败: {e}")))?;
    let mut since_commit: u32 = 0;

    for msg in messages {
        // 只处理 type == "gemini" 的消息
        if msg.get("type").and_then(|t| t.as_str()) != Some("gemini") {
            continue;
        }

        // 提取 tokens 对象
        let tokens_obj = match msg.get("tokens") {
            Some(t) if t.is_object() => t,
            _ => continue,
        };

        let tokens = parse_gemini_tokens(tokens_obj);
        if tokens.input == 0 && tokens.output == 0 && tokens.thoughts == 0 && tokens.cached == 0 {
            continue; // 跳过全零的空 token 消息
        }

        gemini_msg_count += 1;

        // 提取消息 ID 和模型
        let message_id = msg.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
        let model = msg
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let timestamp = msg.get("timestamp").and_then(|v| v.as_str());

        // 生成唯一 request_id
        let session_id_str = session_id.as_deref().unwrap_or("unknown");
        let request_id = format!("gemini_session:{session_id_str}:{message_id}");

        match insert_gemini_session_entry(
            &tx,
            pricing_cache,
            &request_id,
            &tokens,
            model,
            session_id.as_deref(),
            timestamp,
        ) {
            Ok(true) => imported += 1,
            Ok(false) => skipped += 1,
            Err(e) => {
                log::warn!("[GEMINI-SYNC] 插入失败 ({}): {e}", request_id);
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
    update_sync_state_conn(&tx, &file_path_str, file_modified, gemini_msg_count)?;
    tx.commit()
        .map_err(|e| AppError::Database(format!("提交事务失败: {e}")))?;
    drop(guard);

    // 每个文件若有新插入/更新行，只通知一次（旧实现为每行一次）
    if imported > 0 {
        crate::usage_events::notify_log_recorded();
    }

    Ok((imported, skipped))
}

/// 从 tokens JSON 对象中提取 token 数据
fn parse_gemini_tokens(tokens: &serde_json::Value) -> GeminiTokens {
    GeminiTokens {
        input: tokens.get("input").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        output: tokens.get("output").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        cached: tokens.get("cached").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        thoughts: tokens.get("thoughts").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
    }
}

/// 插入单条 Gemini 会话记录到 proxy_request_logs
///
/// 调用方在同一事务连接上批量调用本函数；查询与 UPSERT 走 prepare_cached，
/// 费用查询走 per-cycle 定价缓存。
fn insert_gemini_session_entry(
    conn: &rusqlite::Connection,
    pricing_cache: &mut PricingCache,
    request_id: &str,
    tokens: &GeminiTokens,
    model: &str,
    session_id: Option<&str>,
    timestamp: Option<&str>,
) -> Result<bool, AppError> {
    let created_at = timestamp
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

    // 合并 thoughts 到 output（思考 token 按输出计费）
    let output_tokens = tokens.output + tokens.thoughts;

    let dedup_key = DedupKey {
        app_type: "gemini",
        model,
        input_tokens: tokens.input,
        output_tokens,
        cache_read_tokens: tokens.cached,
        cache_creation_tokens: 0,
        created_at,
    };
    let existing_data_source: Option<String> = {
        let mut stmt = conn
            .prepare_cached(
                "SELECT COALESCE(data_source, 'proxy') FROM proxy_request_logs WHERE request_id = ?1",
            )
            .map_err(|e| AppError::Database(format!("查询 Gemini request_id 失败: {e}")))?;
        stmt.query_row([request_id], |row| row.get(0))
            .optional()
            .map_err(|e| AppError::Database(format!("查询 Gemini request_id 失败: {e}")))?
    };
    let existing_is_gemini_session = existing_data_source.as_deref() == Some("gemini_session");
    if existing_data_source.is_some() && !existing_is_gemini_session {
        return Ok(false);
    }
    if !existing_is_gemini_session && has_matching_proxy_usage_log(conn, &dedup_key)? {
        return Ok(false);
    }

    // 计算费用
    let usage = TokenUsage {
        input_tokens: tokens.input,
        output_tokens,
        cache_read_tokens: tokens.cached,
        cache_creation_tokens: 0,
        model: Some(model.to_string()),
        message_id: None,
    };

    let pricing = cached_model_pricing(conn, pricing_cache, model);
    let multiplier = Decimal::from(1);
    let (input_cost, output_cost, cache_read_cost, cache_creation_cost, total_cost) = match pricing
    {
        Some(p) => {
            let cost = CostCalculator::calculate_for_app("gemini", &usage, &p, multiplier);
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

    // 使用 UPSERT：新记录插入，已存在记录更新 token 和费用（Gemini 全量重读可能携带更新值）
    let mut stmt = conn
        .prepare_cached(
            "INSERT INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at, data_source
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)
        ON CONFLICT(request_id) DO UPDATE SET
            model = excluded.model,
            request_model = excluded.request_model,
            input_tokens = excluded.input_tokens,
            output_tokens = excluded.output_tokens,
            cache_read_tokens = excluded.cache_read_tokens,
            cache_creation_tokens = excluded.cache_creation_tokens,
            input_cost_usd = excluded.input_cost_usd,
            output_cost_usd = excluded.output_cost_usd,
            cache_read_cost_usd = excluded.cache_read_cost_usd,
            cache_creation_cost_usd = excluded.cache_creation_cost_usd,
            total_cost_usd = excluded.total_cost_usd,
            session_id = excluded.session_id,
            created_at = excluded.created_at
        WHERE input_tokens != excluded.input_tokens
           OR output_tokens != excluded.output_tokens
           OR cache_read_tokens != excluded.cache_read_tokens
           OR cache_creation_tokens != excluded.cache_creation_tokens
           OR model != excluded.model",
        )
        .map_err(|e| AppError::Database(format!("插入 Gemini 会话日志失败: {e}")))?;
    // execute 返回值即本条语句改动的行数：INSERT 或命中 WHERE 的 UPDATE 返回 >0，
    // 值完全相同的 UPSERT（WHERE 不成立）返回 0，语义与旧 conn.changes() 一致。
    let changed = stmt
        .execute(rusqlite::params![
            request_id,
            "_gemini_session", // provider_id
            "gemini",          // app_type
            model,
            model, // request_model = model
            tokens.input,
            output_tokens,
            tokens.cached,
            0i64, // cache_creation_tokens
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
            Some("gemini_session"), // provider_type
            1i64,                   // is_streaming
            "1.0",                  // cost_multiplier
            created_at,
            "gemini_session", // data_source
        ])
        .map_err(|e| AppError::Database(format!("插入 Gemini 会话日志失败: {e}")))?
        > 0;

    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_gemini_session_files_nonexistent() {
        let files = collect_gemini_session_files(Path::new("/nonexistent/path"));
        assert!(files.is_empty());
    }

    /// 收集函数返回按 mtime 降序：用显式 set_modified 控制两个文件的先后，断言
    /// 较新的会话文件排在前（最新优先，Usage 默认 Today/7d 尽快出数）。
    #[test]
    fn test_collect_gemini_session_files_orders_newest_first() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let chats = tmp.path().join("tmp").join("proj").join("chats");
        fs::create_dir_all(&chats).expect("create chats dir");

        let older = chats.join("session-old.json");
        let newer = chats.join("session-new.json");
        fs::write(&older, "{}").expect("write older");
        fs::write(&newer, "{}").expect("write newer");

        // 显式设置 mtime：newer 比 older 晚 10 秒（不依赖文件系统时间粒度）。
        let base = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&older)
            .expect("open older")
            .set_modified(base)
            .expect("set older mtime");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&newer)
            .expect("open newer")
            .set_modified(base + std::time::Duration::from_secs(10))
            .expect("set newer mtime");

        let files = collect_gemini_session_files(tmp.path());
        assert_eq!(files.len(), 2);
        // 降序：newer 在前，older 在后。
        assert!(
            files[0].0.ends_with("session-new.json"),
            "最新文件应排在最前: {:?}",
            files[0].0
        );
        assert!(
            files[1].0.ends_with("session-old.json"),
            "较旧文件应排在最后: {:?}",
            files[1].0
        );
        // 携带的 mtime 也应是降序（newer 的 mtime 更大）。
        assert!(files[0].1 > files[1].1, "mtime 应降序");
    }

    #[test]
    fn test_insert_gemini_session_skips_matching_proxy_log() -> Result<(), AppError> {
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
                    "gemini-proxy",
                    "google",
                    "gemini",
                    "gemini-2.5-pro",
                    "gemini-2.5-pro",
                    10,
                    7,
                    1,
                    0,
                    "0.01",
                    100,
                    200,
                    1000,
                    "proxy"
                ],
            )?;
        }

        let tokens = GeminiTokens {
            input: 10,
            output: 2,
            cached: 1,
            thoughts: 5,
        };
        let mut pricing_cache = PricingCache::new();
        let inserted = {
            let conn = lock_conn!(db.conn);
            insert_gemini_session_entry(
                &conn,
                &mut pricing_cache,
                "gemini-session-dup",
                &tokens,
                "gemini-2.5-pro",
                Some("session-1"),
                Some("1970-01-01T00:16:45Z"),
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

    #[test]
    fn test_insert_gemini_session_updates_existing_session_entry() -> Result<(), AppError> {
        let db = Database::memory()?;
        let mut pricing_cache = PricingCache::new();
        let first = GeminiTokens {
            input: 10,
            output: 2,
            cached: 1,
            thoughts: 5,
        };
        assert!({
            let conn = lock_conn!(db.conn);
            insert_gemini_session_entry(
                &conn,
                &mut pricing_cache,
                "gemini-session-updated",
                &first,
                "gemini-2.5-pro",
                Some("session-1"),
                Some("1970-01-01T00:16:45Z"),
            )?
        });

        let updated = GeminiTokens {
            input: 20,
            output: 3,
            cached: 2,
            thoughts: 7,
        };
        assert!({
            let conn = lock_conn!(db.conn);
            insert_gemini_session_entry(
                &conn,
                &mut pricing_cache,
                "gemini-session-updated",
                &updated,
                "gemini-2.5-flash",
                Some("session-1"),
                Some("1970-01-01T00:16:45Z"),
            )?
        });

        let conn = lock_conn!(db.conn);
        let (model, input, output, cached, data_source): (String, i64, i64, i64, String) = conn
            .query_row(
                "SELECT model, input_tokens, output_tokens, cache_read_tokens, data_source
                 FROM proxy_request_logs WHERE request_id = 'gemini-session-updated'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;
        assert_eq!(model, "gemini-2.5-flash");
        assert_eq!(input, 20);
        assert_eq!(output, 10);
        assert_eq!(cached, 2);
        assert_eq!(data_source, "gemini_session");

        Ok(())
    }

    #[test]
    fn test_parse_gemini_tokens() {
        let json: serde_json::Value = serde_json::json!({
            "input": 8522,
            "output": 29,
            "cached": 3138,
            "thoughts": 405,
            "tool": 0,
            "total": 8956
        });
        let tokens = parse_gemini_tokens(&json);
        assert_eq!(tokens.input, 8522);
        assert_eq!(tokens.output, 29);
        assert_eq!(tokens.cached, 3138);
        assert_eq!(tokens.thoughts, 405);
        // output + thoughts = 29 + 405 = 434（用于计费）
        assert_eq!(tokens.output + tokens.thoughts, 434);
    }

    #[test]
    fn test_parse_gemini_tokens_missing_fields() {
        // 缺少某些字段时应返回 0
        let json: serde_json::Value = serde_json::json!({
            "input": 100,
            "output": 50
        });
        let tokens = parse_gemini_tokens(&json);
        assert_eq!(tokens.input, 100);
        assert_eq!(tokens.output, 50);
        assert_eq!(tokens.cached, 0);
        assert_eq!(tokens.thoughts, 0);
    }

    #[test]
    fn test_parse_gemini_tokens_all_zero() {
        let json: serde_json::Value = serde_json::json!({
            "input": 0,
            "output": 0,
            "cached": 0,
            "thoughts": 0,
            "tool": 0,
            "total": 0
        });
        let tokens = parse_gemini_tokens(&json);
        assert_eq!(tokens.input, 0);
        assert_eq!(tokens.output, 0);
        // 全零（包括 cached=0）会被 sync 逻辑跳过
        assert!(
            tokens.input == 0 && tokens.output == 0 && tokens.thoughts == 0 && tokens.cached == 0
        );
    }

    #[test]
    fn test_parse_gemini_tokens_cache_only_not_skipped() {
        // 纯缓存命中消息（input/output/thoughts=0 但 cached>0）不应被跳过
        let json: serde_json::Value = serde_json::json!({
            "input": 0,
            "output": 0,
            "cached": 5000,
            "thoughts": 0
        });
        let tokens = parse_gemini_tokens(&json);
        assert_eq!(tokens.cached, 5000);
        // 跳过条件：所有四个字段都为 0 才跳过
        let should_skip =
            tokens.input == 0 && tokens.output == 0 && tokens.thoughts == 0 && tokens.cached == 0;
        assert!(!should_skip, "纯缓存命中记录不应被跳过");
    }
}
