//! 会话扫描元数据的 sidecar 缓存库。
//!
//! 主库 cc-switch.db 的 schema 与上游项目同步（WebDAV 亦会整库同步到其他
//! 机器），本仓库不得自行加表；而本缓存存的是机器本地的绝对路径且完全可
//! 重建，因此放在独立的 `session-scan-cache.db` 文件里：不参与任何同步，
//! 也无需版本化迁移——打开时幂等建表，结构不兼容时靠 `cache_version` 列
//! 整体失效（见 [`crate::session_manager::cache::SCAN_CACHE_VERSION`]）。
//!
//! 这是纯缓存：任何打开/读/写失败都应由调用方降级为"无缓存"（全量解析），
//! 绝不让缓存故障影响会话扫描本身。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::Connection;

use crate::config::{get_app_config_dir, resolve_config_dir_without_following_user_symlinks};
use crate::error::AppError;
use crate::session_manager::cache::{CachedScanRow, SessionScanCacheEntry};

/// sidecar 缓存库文件名（与主库同目录）。
const SCAN_CACHE_DB_FILE: &str = "session-scan-cache.db";

/// 会话扫描缓存的 sidecar SQLite 存储。
///
/// 与主库互不相干：自持连接、自建表，损坏时可直接删除文件重建。
pub struct ScanCacheStore {
    conn: Mutex<Connection>,
}

/// 使用统计增量同步的字节续传提示（存于 sidecar 的 `session_sync_resume` 表）。
///
/// 主库 `session_log_sync` 的 `(last_modified, last_line_offset)` 仍是权威进度；
/// 本提示只是加速手段：`(last_modified, last_line_offset)` 与权威行完全一致、
/// Unix inode（`file_identity`）未变、且 `byte_offset` 前的尾部字节指纹
/// （`tail_hash`）与文件当前内容吻合时，解析器才直接 seek 到 `byte_offset`
/// 续读。任何不一致（整库从别的机器同步进来、文件被截断、inode 变化的
/// rename-replace 类重写）都应忽略提示回退旧路径。
#[derive(Debug, Clone, PartialEq)]
pub struct SyncResumeHint {
    pub file_path: String,
    /// 对应主库 session_log_sync.last_modified 的快照。
    pub last_modified: i64,
    /// 对应主库 session_log_sync.last_line_offset 的快照。
    pub last_line_offset: i64,
    /// 上次处理完成时的字节位置（换行边界，不含末尾不完整行）。
    pub byte_offset: i64,
    /// 解析器续传状态 JSON（Codex 存整个状态机；Claude 存 session_id）。
    pub state: Option<String>,
    /// `byte_offset` 前至多 64 字节的 FNV-1a 指纹。**append-only 假设下**的
    /// 内容边界校验：只能识破"边界前 64 字节发生变化"的原地重写；若文件被
    /// 整体重写成更大文件、而旧 offset 前 64 字节恰好逐字节不变，则识别不出
    /// （非严格保证，会话日志 app 不会原地重写自己的历史）。inode 变化的
    /// rename-replace 类重写另由 `file_identity` 关闭。None 视为无指纹。
    pub tail_hash: Option<i64>,
    /// 上轮结束时"边界之后未终结尾部"的字节数（None = 无待确认尾部）。
    /// 与 `pending_tail_hash` 一起做尾部稳定性确认：对"永远不带换行的最终
    /// 行"，两轮之间尾部字节不变即可收敛，不再每周期复查（见驱动）。
    pub pending_tail_len: Option<i64>,
    /// 上轮未终结尾部（`byte_offset`→EOF）字节的 FNV-1a 指纹。
    pub pending_tail_hash: Option<i64>,
    /// 保存提示时文件的身份标识：Unix 存 inode（`ino() as i64`，位模式保留、
    /// 允许负值），其他平台存 None。续传前若当前文件 inode 与此不同，说明这
    /// 已不是权威 offset 描述的那个文件（rename-replace 类重写、跨机器同步
    /// 进来的异源文件）→ 全量重扫。None（旧提示/非 Unix）时该校验跳过，退回
    /// tail_hash 启发式。
    pub file_identity: Option<i64>,
}

impl ScanCacheStore {
    /// 打开（必要时创建）配置目录下的 sidecar 缓存库。
    pub fn open() -> Result<Self, AppError> {
        let config_dir = resolve_config_dir_without_following_user_symlinks(&get_app_config_dir())?;
        Self::open_at(&config_dir.join(SCAN_CACHE_DB_FILE))
    }

    /// 在指定路径打开缓存库（测试与 `open()` 共用）。
    pub fn open_at(path: &Path) -> Result<Self, AppError> {
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
            }
        }
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let conn = Connection::open_with_flags(path, flags)
            .map_err(|e| AppError::Database(format!("打开会话扫描缓存库失败: {e}")))?;
        // 与主库一致：缓存虽可重建，但含会话元数据与绝对路径，unix 下收紧为 0600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Self::from_connection(conn)
    }

    /// 内存缓存库（仅测试用）。
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, AppError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| AppError::Database(format!("打开内存缓存库失败: {e}")))?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self, AppError> {
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| AppError::Database(e.to_string()))?;
        // 与主库一致：WAL 降低写阻塞，NORMAL 免去逐次 COMMIT 的 fsync；
        // 纯缓存丢最新事务毫无影响。
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");

        conn.execute(
            "CREATE TABLE IF NOT EXISTS session_scan_cache (
                file_path TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                mtime_ns INTEGER NOT NULL,
                size INTEGER NOT NULL,
                meta TEXT NOT NULL,
                cache_version INTEGER NOT NULL
            )",
            [],
        )
        .map_err(|e| AppError::Database(format!("创建 session_scan_cache 表失败: {e}")))?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_session_scan_cache_provider
             ON session_scan_cache(provider)",
            [],
        )
        .map_err(|e| AppError::Database(format!("创建 session_scan_cache 索引失败: {e}")))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS session_sync_resume (
                file_path TEXT PRIMARY KEY,
                last_modified INTEGER NOT NULL,
                last_line_offset INTEGER NOT NULL,
                byte_offset INTEGER NOT NULL,
                state TEXT,
                tail_hash INTEGER,
                pending_tail_len INTEGER,
                pending_tail_hash INTEGER,
                file_identity INTEGER
            )",
            [],
        )
        .map_err(|e| AppError::Database(format!("创建 session_sync_resume 表失败: {e}")))?;
        // 纯本地缓存库无版本化迁移：旧文件缺列时就地补列，失败（列已存在）忽略。
        let _ = conn.execute(
            "ALTER TABLE session_sync_resume ADD COLUMN tail_hash INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE session_sync_resume ADD COLUMN pending_tail_len INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE session_sync_resume ADD COLUMN pending_tail_hash INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE session_sync_resume ADD COLUMN file_identity INTEGER",
            [],
        );

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// sidecar 缓存库的磁盘路径（不打开连接；诊断/清理用）。
    #[allow(dead_code)]
    pub fn path() -> Result<PathBuf, AppError> {
        let config_dir = resolve_config_dir_without_following_user_symlinks(&get_app_config_dir())?;
        Ok(config_dir.join(SCAN_CACHE_DB_FILE))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, AppError> {
        self.conn
            .lock()
            .map_err(|_| AppError::Database("会话扫描缓存库连接锁中毒".to_string()))
    }

    /// 读取某个 provider 的全部缓存行，键为绝对文件路径。
    pub fn load_for_provider(
        &self,
        provider: &str,
    ) -> Result<HashMap<String, CachedScanRow>, AppError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT file_path, mtime_ns, size, cache_version, meta
                 FROM session_scan_cache
                 WHERE provider = ?1",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([provider], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    CachedScanRow {
                        mtime_ns: row.get::<_, i64>(1)?,
                        size: row.get::<_, i64>(2)?,
                        cache_version: row.get::<_, i64>(3)?,
                        meta_json: row.get::<_, String>(4)?,
                    },
                ))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;

        let mut map = HashMap::new();
        for row in rows {
            let (file_path, cached) = row.map_err(|e| AppError::Database(e.to_string()))?;
            map.insert(file_path, cached);
        }
        Ok(map)
    }

    /// 读取缓存的 `SessionMeta` JSON（用于秒开快照）。`provider` 为 `None` 时返回
    /// 全部 provider 的行；只返回 `cache_version` 匹配当前版本的行，让整体版本失效
    /// 时快照自动为空、退回到全量扫描。
    pub fn load_meta_json(
        &self,
        provider: Option<&str>,
        cache_version: i64,
    ) -> Result<Vec<String>, AppError> {
        let conn = self.lock()?;
        let mut out = Vec::new();
        match provider {
            Some(provider) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT meta FROM session_scan_cache
                         WHERE provider = ?1 AND cache_version = ?2",
                    )
                    .map_err(|e| AppError::Database(e.to_string()))?;
                let rows = stmt
                    .query_map(rusqlite::params![provider, cache_version], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(|e| AppError::Database(e.to_string()))?;
                for row in rows {
                    out.push(row.map_err(|e| AppError::Database(e.to_string()))?);
                }
            }
            None => {
                let mut stmt = conn
                    .prepare("SELECT meta FROM session_scan_cache WHERE cache_version = ?1")
                    .map_err(|e| AppError::Database(e.to_string()))?;
                let rows = stmt
                    .query_map([cache_version], |row| row.get::<_, String>(0))
                    .map_err(|e| AppError::Database(e.to_string()))?;
                for row in rows {
                    out.push(row.map_err(|e| AppError::Database(e.to_string()))?);
                }
            }
        }
        Ok(out)
    }

    /// 在单个事务里批量写入（新增/更新）缓存行。
    pub fn upsert_batch(&self, entries: &[SessionScanCacheEntry]) -> Result<(), AppError> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock()?;
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO session_scan_cache
                        (file_path, provider, mtime_ns, size, meta, cache_version)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(file_path) DO UPDATE SET
                        provider = excluded.provider,
                        mtime_ns = excluded.mtime_ns,
                        size = excluded.size,
                        meta = excluded.meta,
                        cache_version = excluded.cache_version",
                )
                .map_err(|e| AppError::Database(e.to_string()))?;
            for entry in entries {
                stmt.execute(rusqlite::params![
                    entry.file_path,
                    entry.provider,
                    entry.mtime_ns,
                    entry.size,
                    entry.meta_json,
                    entry.cache_version,
                ])
                .map_err(|e| AppError::Database(e.to_string()))?;
            }
        }
        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 在单个事务里批量删除给定路径的缓存行。
    pub fn delete_paths(&self, paths: &[String]) -> Result<(), AppError> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock()?;
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(e.to_string()))?;
        {
            let mut stmt = tx
                .prepare_cached("DELETE FROM session_scan_cache WHERE file_path = ?1")
                .map_err(|e| AppError::Database(e.to_string()))?;
            for path in paths {
                stmt.execute([path])
                    .map_err(|e| AppError::Database(e.to_string()))?;
            }
        }
        tx.commit().map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 按 provider + session_id 删除缓存行（兜底 opencode 等 source_path ≠
    /// 缓存键的 provider）。
    ///
    /// 缓存主键是 session 文件的绝对路径（walk 时的 `target.path`），而 opencode
    /// 的 `meta.source_path` 是 message 目录，二者不同——按路径删
    /// （[`delete_paths`](Self::delete_paths)）对 opencode 是 no-op。这里改用
    /// meta JSON 里的 `sessionId` 兜底：`SessionMeta` serde 为 camelCase，
    /// `serde_json::to_string` 产出无空格的紧凑 JSON，其中恰为
    /// `"sessionId":"<session_id>"`。session_id 来自内部数据、不含通配符，但仍
    /// 对 LIKE 的 `%`/`_`/`\` 做转义以稳妥（配合 `ESCAPE '\'`）。
    pub fn delete_rows_by_session(&self, provider: &str, session_id: &str) -> Result<(), AppError> {
        let pattern = format!("%\"sessionId\":\"{}\"%", escape_like(session_id));
        let conn = self.lock()?;
        conn.execute(
            "DELETE FROM session_scan_cache
             WHERE provider = ?1 AND meta LIKE ?2 ESCAPE '\\'",
            rusqlite::params![provider, pattern],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 清空某个 provider 的全部缓存行。
    #[allow(dead_code)]
    pub fn clear_provider(&self, provider: &str) -> Result<(), AppError> {
        let conn = self.lock()?;
        conn.execute(
            "DELETE FROM session_scan_cache WHERE provider = ?1",
            [provider],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 一次性读取整张 `session_sync_resume` 表，键为绝对文件路径。
    ///
    /// fix 2：让上万文件的增量同步在 skip 前的身份校验走内存查找，而非逐文件
    /// 查库（类比 [`crate::services::session_usage::get_all_sync_states`]）。稳态下
    /// 顺带把"每变化文件一次 hint 查询"收敛为每周期一次全表查询。读失败由调用方
    /// 降级为空表（视作无提示，全部回退旧路径），不影响正确性。
    pub fn load_all_sync_resume(&self) -> Result<HashMap<String, SyncResumeHint>, AppError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT file_path, last_modified, last_line_offset, byte_offset, state, tail_hash,
                        pending_tail_len, pending_tail_hash, file_identity
                 FROM session_sync_resume",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                let file_path: String = row.get(0)?;
                Ok((
                    file_path.clone(),
                    SyncResumeHint {
                        file_path,
                        last_modified: row.get(1)?,
                        last_line_offset: row.get(2)?,
                        byte_offset: row.get(3)?,
                        state: row.get(4)?,
                        tail_hash: row.get(5)?,
                        pending_tail_len: row.get(6)?,
                        pending_tail_hash: row.get(7)?,
                        file_identity: row.get(8)?,
                    },
                ))
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        let mut map = HashMap::new();
        for row in rows {
            let (path, hint) = row.map_err(|e| AppError::Database(e.to_string()))?;
            map.insert(path, hint);
        }
        Ok(map)
    }

    /// 读取某个文件的字节续传提示。fix 2 后生产侧改用 [`Self::load_all_sync_resume`]
    /// 预载，此单文件查询保留作 API 对称与测试用途。
    #[allow(dead_code)]
    pub fn load_sync_resume(&self, file_path: &str) -> Result<Option<SyncResumeHint>, AppError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT last_modified, last_line_offset, byte_offset, state, tail_hash,
                        pending_tail_len, pending_tail_hash, file_identity
                 FROM session_sync_resume WHERE file_path = ?1",
            )
            .map_err(|e| AppError::Database(e.to_string()))?;
        let hint = stmt
            .query_row([file_path], |row| {
                Ok(SyncResumeHint {
                    file_path: file_path.to_string(),
                    last_modified: row.get(0)?,
                    last_line_offset: row.get(1)?,
                    byte_offset: row.get(2)?,
                    state: row.get(3)?,
                    tail_hash: row.get(4)?,
                    pending_tail_len: row.get(5)?,
                    pending_tail_hash: row.get(6)?,
                    file_identity: row.get(7)?,
                })
            })
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(AppError::Database(other.to_string())),
            })?;
        Ok(hint)
    }

    /// 写入（覆盖）某个文件的字节续传提示。
    pub fn save_sync_resume(&self, hint: &SyncResumeHint) -> Result<(), AppError> {
        let conn = self.lock()?;
        // 单调更新：并发同步中较晚提交的旧快照不得覆盖较新的提示。
        // mtime 粒度有限，相等时按 byte_offset 单调判定（与主库进度的
        // (mtime, offset) 字典序规则一致）。
        conn.execute(
            "INSERT INTO session_sync_resume
                (file_path, last_modified, last_line_offset, byte_offset, state, tail_hash,
                 pending_tail_len, pending_tail_hash, file_identity)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(file_path) DO UPDATE SET
                last_modified = excluded.last_modified,
                last_line_offset = excluded.last_line_offset,
                byte_offset = excluded.byte_offset,
                state = excluded.state,
                tail_hash = excluded.tail_hash,
                pending_tail_len = excluded.pending_tail_len,
                pending_tail_hash = excluded.pending_tail_hash,
                file_identity = excluded.file_identity
             WHERE excluded.last_modified > session_sync_resume.last_modified
                OR (excluded.last_modified = session_sync_resume.last_modified
                    AND excluded.byte_offset >= session_sync_resume.byte_offset)",
            rusqlite::params![
                hint.file_path,
                hint.last_modified,
                hint.last_line_offset,
                hint.byte_offset,
                hint.state,
                hint.tail_hash,
                hint.pending_tail_len,
                hint.pending_tail_hash,
                hint.file_identity,
            ],
        )
        .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

/// 删除会话成功后清理 sidecar 扫描缓存里对应的行。纯缓存操作：打开/删除失败
/// 只记 debug，绝不影响删除结果本身。
///
/// TUI 删除 worker 与 CLI `sessions delete` 成功路径共用此函数，保证两个入口
/// 一致地清缓存——否则删完再开另一入口时，stale-while-revalidate 的秒开快照会
/// 让已删会话短暂"复活"（要等后台重扫的 deletes 才自愈）。内部自行 `open()`
/// sidecar；打不开就降级为无操作（下一轮 revalidate 仍会靠文件缺失自愈）。
pub fn purge_session(provider_id: &str, session_id: &str, source_path: &str) {
    match ScanCacheStore::open() {
        Ok(store) => purge_session_in(&store, provider_id, session_id, source_path),
        Err(err) => {
            log::debug!("[SESSION-SCAN] 删除会话后打开扫描缓存失败 ({source_path}): {err}")
        }
    }
}

/// [`purge_session`] 的核心两步删除（在已打开的 store 上执行）。抽出来让测试可
/// 直接用内存 store 覆盖，不落磁盘。
///
/// 1. 按 `source_path` 删（[`ScanCacheStore::delete_paths`]）——覆盖 claude/codex/
///    gemini/openclaw，其 `source_path` 即缓存主键（会话文件路径）。
/// 2. 按 `session_id` 删（[`ScanCacheStore::delete_rows_by_session`]）——兜底
///    opencode 及未来 `source_path` ≠ 缓存键的 provider：opencode 的 `source_path`
///    是 message 目录（非缓存键，缓存键是 session JSON 路径），仅按路径删是
///    no-op；改用 meta JSON 里的 `sessionId` 精确删到那一行。
pub(crate) fn purge_session_in(
    store: &ScanCacheStore,
    provider_id: &str,
    session_id: &str,
    source_path: &str,
) {
    if let Err(err) = store.delete_paths(&[source_path.to_string()]) {
        log::debug!("[SESSION-SCAN] 删除会话后按路径清理扫描缓存失败 ({source_path}): {err}");
    }
    if let Err(err) = store.delete_rows_by_session(provider_id, session_id) {
        log::debug!(
            "[SESSION-SCAN] 删除会话后按 sessionId 清理扫描缓存失败 ({provider_id}/{session_id}): {err}"
        );
    }
}

/// 转义 LIKE 模式中的通配符（`\`/`%`/`_`），配合 `ESCAPE '\'` 使用，使字面量
/// 精确匹配、不被当作通配符。
fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_manager::cache::SCAN_CACHE_VERSION;

    fn entry(
        path: &str,
        provider: &str,
        mtime: i64,
        size: i64,
        version: i64,
    ) -> SessionScanCacheEntry {
        SessionScanCacheEntry {
            file_path: path.to_string(),
            provider: provider.to_string(),
            mtime_ns: mtime,
            size,
            meta_json: format!("{{\"providerId\":\"{provider}\",\"sessionId\":\"{path}\"}}"),
            cache_version: version,
        }
    }

    #[test]
    fn upsert_load_and_delete_roundtrip() {
        let store = ScanCacheStore::in_memory().expect("open memory store");
        store
            .upsert_batch(&[
                entry("/a.jsonl", "claude", 1, 10, SCAN_CACHE_VERSION),
                entry("/b.jsonl", "claude", 2, 20, SCAN_CACHE_VERSION),
                entry("/c.jsonl", "codex", 3, 30, SCAN_CACHE_VERSION),
            ])
            .expect("upsert");

        let claude = store.load_for_provider("claude").expect("load");
        assert_eq!(claude.len(), 2);
        assert_eq!(claude.get("/a.jsonl").unwrap().mtime_ns, 1);

        // Upsert replaces an existing row (same primary key).
        store
            .upsert_batch(&[entry("/a.jsonl", "claude", 99, 10, SCAN_CACHE_VERSION)])
            .expect("upsert replace");
        let claude = store.load_for_provider("claude").expect("reload");
        assert_eq!(claude.get("/a.jsonl").unwrap().mtime_ns, 99);

        store
            .delete_paths(&["/a.jsonl".to_string()])
            .expect("delete");
        let claude = store
            .load_for_provider("claude")
            .expect("reload after delete");
        assert_eq!(claude.len(), 1);
        assert!(claude.contains_key("/b.jsonl"));
    }

    #[test]
    fn meta_json_snapshot_filters_by_provider_and_version() {
        let store = ScanCacheStore::in_memory().expect("open memory store");
        store
            .upsert_batch(&[
                entry("/a.jsonl", "claude", 1, 10, SCAN_CACHE_VERSION),
                entry("/b.jsonl", "codex", 2, 20, SCAN_CACHE_VERSION),
                entry("/old.jsonl", "claude", 3, 30, SCAN_CACHE_VERSION - 1),
            ])
            .expect("upsert");

        let claude = store
            .load_meta_json(Some("claude"), SCAN_CACHE_VERSION)
            .expect("load claude");
        assert_eq!(claude.len(), 1); // old-version row is excluded

        let all = store
            .load_meta_json(None, SCAN_CACHE_VERSION)
            .expect("load all");
        assert_eq!(all.len(), 2);
    }

    /// 按 sessionId 删除只影响目标行；LIKE 通配符（`_`）经转义后按字面量匹配，
    /// 不会误删 sessionId 仅相差一字符的行。
    #[test]
    fn delete_rows_by_session_removes_only_matching_session_id() {
        let store = ScanCacheStore::in_memory().expect("open memory store");
        // entry() 的 meta_json 形如 {"providerId":"<p>","sessionId":"<path>"}，
        // 这里用 file_path 值充当 sessionId。ses_a 与 sesXa 仅一字符之差，若不
        // 转义 `_`，删 ses_a 会把 sesXa 一并误删。
        store
            .upsert_batch(&[
                entry("ses_a", "opencode", 1, 10, SCAN_CACHE_VERSION),
                entry("sesXa", "opencode", 2, 20, SCAN_CACHE_VERSION),
            ])
            .expect("upsert");

        store
            .delete_rows_by_session("opencode", "ses_a")
            .expect("delete by session");

        let rows = store.load_for_provider("opencode").expect("load");
        assert_eq!(rows.len(), 1);
        assert!(
            rows.contains_key("sesXa"),
            "`_` 经转义按字面量匹配，sesXa 应保留"
        );
    }

    /// 删除会话成功后 `purge_session_in` 按路径清掉 sidecar 缓存行，避免下次
    /// 秒开快照复活已删会话。缓存里没有的路径为无害 no-op。
    #[test]
    fn purge_session_in_removes_deleted_session_by_path() {
        let store = ScanCacheStore::in_memory().expect("open memory store");
        store
            .upsert_batch(&[entry(
                "/tmp/gone.jsonl",
                "claude",
                1,
                10,
                SCAN_CACHE_VERSION,
            )])
            .expect("upsert");
        assert_eq!(store.load_for_provider("claude").expect("load").len(), 1);

        purge_session_in(&store, "claude", "gone", "/tmp/gone.jsonl");
        assert!(
            store.load_for_provider("claude").expect("load").is_empty(),
            "已删除会话的缓存行应被按路径清除"
        );

        // 不存在的路径/会话不 panic、不报错（无害 no-op）。
        purge_session_in(&store, "claude", "nope", "/tmp/never-existed.jsonl");
    }

    /// opencode 的 `source_path` 是 message 目录、≠ 缓存主键（session JSON
    /// 路径），仅按路径删是 no-op；`purge_session_in` 会额外按 `sessionId` 兜底
    /// 删除该行。
    #[test]
    fn purge_session_in_removes_opencode_row_by_session_id() {
        let store = ScanCacheStore::in_memory().expect("open memory store");
        // 缓存主键是 session JSON 文件路径；meta 里带 sessionId。
        store
            .upsert_batch(&[SessionScanCacheEntry {
                file_path: "/data/opencode/storage/session/proj/ses_x.json".to_string(),
                provider: "opencode".to_string(),
                mtime_ns: 1,
                size: 10,
                meta_json: r#"{"providerId":"opencode","sessionId":"ses_x"}"#.to_string(),
                cache_version: SCAN_CACHE_VERSION,
            }])
            .expect("upsert");

        // Delete 请求携带的 source_path 是 message 目录（≠ 缓存键）：仅按路径
        // 删除会是 no-op，必须靠 sessionId 兜底。
        purge_session_in(
            &store,
            "opencode",
            "ses_x",
            "/data/opencode/storage/message/ses_x",
        );

        assert!(
            store
                .load_for_provider("opencode")
                .expect("load")
                .is_empty(),
            "opencode 行应按 sessionId 兜底删除"
        );
    }

    #[test]
    fn clear_provider_removes_only_that_provider() {
        let store = ScanCacheStore::in_memory().expect("open memory store");
        store
            .upsert_batch(&[
                entry("/a.jsonl", "claude", 1, 10, SCAN_CACHE_VERSION),
                entry("/c.jsonl", "codex", 3, 30, SCAN_CACHE_VERSION),
            ])
            .expect("upsert");

        store.clear_provider("claude").expect("clear");
        assert!(store.load_for_provider("claude").expect("load").is_empty());
        assert_eq!(store.load_for_provider("codex").expect("load").len(), 1);
    }

    /// 提示写入按 (mtime, byte_offset) 字典序单调，与主库进度规则一致。
    #[test]
    fn sync_resume_hints_are_monotonic() {
        let store = ScanCacheStore::in_memory().expect("open memory store");
        let hint = |mtime: i64, byte: i64| SyncResumeHint {
            file_path: "/f.jsonl".to_string(),
            last_modified: mtime,
            last_line_offset: byte / 10,
            byte_offset: byte,
            state: None,
            tail_hash: Some(1),
            pending_tail_len: None,
            pending_tail_hash: None,
            file_identity: None,
        };

        store.save_sync_resume(&hint(5, 120)).expect("save");
        store.save_sync_resume(&hint(5, 100)).expect("stale byte");
        store.save_sync_resume(&hint(4, 999)).expect("stale mtime");
        let loaded = store
            .load_sync_resume("/f.jsonl")
            .expect("load")
            .expect("hint");
        assert_eq!((loaded.last_modified, loaded.byte_offset), (5, 120));

        store.save_sync_resume(&hint(5, 130)).expect("newer byte");
        store.save_sync_resume(&hint(6, 10)).expect("newer mtime");
        let loaded = store
            .load_sync_resume("/f.jsonl")
            .expect("load")
            .expect("hint");
        assert_eq!((loaded.last_modified, loaded.byte_offset), (6, 10));
    }

    /// pending_tail 两列随提示一起往返读写（尾部稳定性确认所需）。
    #[test]
    fn sync_resume_hint_roundtrips_pending_tail() {
        let store = ScanCacheStore::in_memory().expect("open memory store");
        let hint = SyncResumeHint {
            file_path: "/p.jsonl".to_string(),
            last_modified: 10,
            last_line_offset: 2,
            byte_offset: 20,
            state: Some("{}".to_string()),
            tail_hash: Some(7),
            pending_tail_len: Some(5),
            pending_tail_hash: Some(1234),
            file_identity: Some(9988),
        };
        store.save_sync_resume(&hint).expect("save");
        let loaded = store
            .load_sync_resume("/p.jsonl")
            .expect("load")
            .expect("hint");
        assert_eq!(loaded.pending_tail_len, Some(5));
        assert_eq!(loaded.pending_tail_hash, Some(1234));
        assert_eq!(
            loaded.file_identity,
            Some(9988),
            "file_identity 应随提示往返"
        );

        // 收敛写回：更大的 mtime 覆盖，pending_tail 清空为 NULL。
        let cleared = SyncResumeHint {
            last_modified: 11,
            pending_tail_len: None,
            pending_tail_hash: None,
            ..hint
        };
        store.save_sync_resume(&cleared).expect("save cleared");
        let loaded = store
            .load_sync_resume("/p.jsonl")
            .expect("load")
            .expect("hint");
        assert_eq!(loaded.pending_tail_len, None);
        assert_eq!(loaded.pending_tail_hash, None);
    }

    /// fix 2：一次全表读回整张 session_sync_resume（键为路径），供预载后内存查找。
    #[test]
    fn load_all_sync_resume_returns_every_row() {
        let store = ScanCacheStore::in_memory().expect("open memory store");
        let hint = |path: &str, mtime: i64| SyncResumeHint {
            file_path: path.to_string(),
            last_modified: mtime,
            last_line_offset: 2,
            byte_offset: 20,
            state: Some("{}".to_string()),
            tail_hash: Some(7),
            pending_tail_len: None,
            pending_tail_hash: None,
            file_identity: Some(4242),
        };
        store
            .save_sync_resume(&hint("/a.jsonl", 10))
            .expect("save a");
        store
            .save_sync_resume(&hint("/b.jsonl", 20))
            .expect("save b");

        let all = store.load_all_sync_resume().expect("load all");
        assert_eq!(all.len(), 2);
        assert_eq!(all.get("/a.jsonl").unwrap().last_modified, 10);
        assert_eq!(all.get("/b.jsonl").unwrap().file_identity, Some(4242));
    }

    #[test]
    fn open_at_creates_missing_parent_and_persists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("session-scan-cache.db");

        let store = ScanCacheStore::open_at(&path).expect("open");
        store
            .upsert_batch(&[entry("/a.jsonl", "claude", 1, 10, SCAN_CACHE_VERSION)])
            .expect("upsert");
        drop(store);

        // 重新打开同一文件应能读回已写入的行（幂等建表不清空数据）。
        let store = ScanCacheStore::open_at(&path).expect("reopen");
        assert_eq!(store.load_for_provider("claude").expect("load").len(), 1);
    }

    #[test]
    fn open_at_corrupt_file_reports_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session-scan-cache.db");
        std::fs::write(&path, b"this is not a sqlite database, definitely").expect("write junk");

        // 打开或建表失败都必须以 Err 返回，交由调用方降级为无缓存扫描。
        let result = ScanCacheStore::open_at(&path)
            .and_then(|store| store.load_for_provider("claude").map(|_| store));
        assert!(result.is_err());
    }
}
