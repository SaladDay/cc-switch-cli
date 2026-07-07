//! Persistent session-scan metadata cache (stale-while-revalidate).
//!
//! The Sessions page used to re-read the head/tail of every session file on each
//! process start; the only cache was in TUI process memory. This module backs the
//! scan with a SQLite table (`session_scan_cache`) keyed on the absolute file
//! path, storing `(mtime_ns, size)` plus the parsed [`SessionMeta`] as JSON.
//!
//! On a subsequent launch the scan only needs one `stat` per file: files whose
//! `(mtime_ns, size)` are unchanged reuse the cached metadata verbatim, so the
//! disk work becomes proportional to changed files rather than to the whole
//! history. Only file-parse-backed providers use this cache; SQLite-only sources
//! (opencode.db / hermes state.db) are a single query and stay uncached.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::session_manager::scan_cache_store::ScanCacheStore;
use crate::session_manager::SessionMeta;

/// Version tag written with every cached row. Bump this constant whenever the
/// cached shape of [`SessionMeta`] changes in a way that field-level
/// `#[serde(default)]` tolerance cannot absorb; rows carrying an older version
/// are ignored on read and re-parsed (then overwritten) on the next scan, so the
/// whole cache invalidates without a schema migration.
pub const SCAN_CACHE_VERSION: i64 = 1;

/// One session file discovered on disk, described by a single `stat`.
#[derive(Debug, Clone)]
pub struct FileScanTarget {
    pub path: PathBuf,
    pub mtime_ns: i64,
    pub size: i64,
}

/// One row read back from the persistent cache.
#[derive(Debug, Clone)]
pub struct CachedScanRow {
    pub mtime_ns: i64,
    pub size: i64,
    pub cache_version: i64,
    pub meta_json: String,
}

/// One row to persist after (re)parsing a session file.
#[derive(Debug, Clone)]
pub struct SessionScanCacheEntry {
    pub file_path: String,
    pub provider: String,
    pub mtime_ns: i64,
    pub size: i64,
    pub meta_json: String,
    pub cache_version: i64,
}

/// The result of reconciling the on-disk files with the cached rows.
#[derive(Debug, Default)]
pub struct ScanDelta {
    /// The full, merged session list (cache hits plus freshly parsed files).
    pub sessions: Vec<SessionMeta>,
    /// Rows to write back (new or changed files that parsed successfully).
    pub upserts: Vec<SessionScanCacheEntry>,
    /// Cache keys to remove (files that disappeared or no longer parse).
    pub deletes: Vec<String>,
    /// Number of sessions returned this round that were deliberately not cached
    /// (the `cacheable` predicate rejected them, e.g. OpenCode no-title sessions
    /// whose summary derives from sibling `part/` files the fingerprint cannot
    /// track). They re-parse every scan; surfaced only for observability logging.
    pub uncacheable: usize,
}

/// `stat` a single path, returning its `(mtime_ns, size)`. Returns `None` when the
/// path is missing or is not a regular file. An unreadable modification time falls
/// back to `0`, which never matches a cached row and so forces a re-parse.
pub fn stat_target(path: &Path) -> Option<FileScanTarget> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    Some(FileScanTarget {
        path: path.to_path_buf(),
        mtime_ns,
        size: meta.len() as i64,
    })
}

/// Mix a sibling dependency's `(mtime, size)` into a target's fingerprint.
///
/// Some providers derive parts of `SessionMeta` from files *next to* the
/// session file (Gemini's `.project_root`, OpenClaw's `sessions.json`
/// display-name map, OpenCode's per-session message directory). The cache
/// fingerprint must change when those change too, or the cached row keeps
/// serving stale derived fields until a manual reload. Missing siblings are
/// simply not mixed in — their later appearance changes the fingerprint.
/// Works for directories as well (a directory's mtime changes when entries
/// are added or removed).
pub fn mix_sibling_into_fingerprint(target: &mut FileScanTarget, sibling: &Path) {
    let Ok(meta) = std::fs::metadata(sibling) else {
        return;
    };
    let sibling_mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    target.mtime_ns = target.mtime_ns.max(sibling_mtime);
    target.size = target.size.wrapping_add(meta.len() as i64);
}

/// Recursively collect files whose extension equals `ext`, statting each once.
/// Mirrors the directory walks the file scanners already use, but reads only
/// metadata (readdir + stat) rather than opening file contents.
pub fn collect_targets_recursive(root: &Path, ext: &str, out: &mut Vec<FileScanTarget>) {
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
            collect_targets_recursive(&path, ext, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            if let Some(target) = stat_target(&path) {
                out.push(target);
            }
        }
    }
}

/// Reconcile the freshly-`stat`ed `targets` against the `cached` rows.
///
/// A target reuses its cached [`SessionMeta`] only when (unless `force`) its
/// cache version matches [`SCAN_CACHE_VERSION`] and its `(mtime_ns, size)` are
/// unchanged and the stored JSON still deserializes; otherwise it is re-parsed.
/// Cached keys that no longer keep a live session — the file vanished, or a
/// re-parse yielded nothing — are collected for deletion.
///
/// `cacheable` gates whether a parsed [`SessionMeta`] may be cached at all. Most
/// providers pass `|_| true`; OpenCode passes `|m| m.title.is_some()` because a
/// no-title session's summary is derived from sibling `part/` files that the
/// `(mtime_ns, size)` fingerprint cannot observe — such a row would otherwise
/// serve a stale summary indefinitely. An uncacheable row still enters the
/// returned list but is never upserted, and any pre-existing cached row for its
/// path is deleted so the next scan re-parses it instead of hitting a stale row.
pub fn revalidate<F, C>(
    provider: &str,
    targets: Vec<FileScanTarget>,
    cached: HashMap<String, CachedScanRow>,
    force: bool,
    parse: F,
    cacheable: C,
) -> ScanDelta
where
    F: Fn(&Path) -> Option<SessionMeta> + Sync,
    C: Fn(&SessionMeta) -> bool + Sync,
{
    let mut sessions = Vec::new();
    let mut keep: HashSet<String> = HashSet::new();
    let mut to_parse: Vec<FileScanTarget> = Vec::new();

    for target in targets {
        let key = target.path.to_string_lossy().to_string();
        if !force {
            if let Some(row) = cached.get(&key) {
                if row.cache_version == SCAN_CACHE_VERSION
                    && row.mtime_ns == target.mtime_ns
                    && row.size == target.size
                {
                    if let Ok(meta) = serde_json::from_str::<SessionMeta>(&row.meta_json) {
                        // 命中捷径仅对可缓存行有效：升级前遗留的不可缓存行
                        // （如曾被误缓存的无 title 会话）不走捷径，落到重新
                        // 解析，随后旧行走 deletes 被清除。
                        if cacheable(&meta) {
                            sessions.push(meta);
                            keep.insert(key);
                            continue;
                        }
                    }
                }
            }
        }
        to_parse.push(target);
    }

    // Parse only the new/changed files, reusing the parallel fan-out so the
    // first-ever run (empty cache → every file parses) keeps today's throughput.
    let parsed = parse_targets_parallel(&to_parse, &parse);
    let mut upserts = Vec::new();
    let mut uncacheable = 0usize;
    for (target, meta) in to_parse.into_iter().zip(parsed) {
        let key = target.path.to_string_lossy().to_string();
        let Some(meta) = meta else {
            continue; // not a session file; leave it out of `keep` so any stale row is deleted
        };
        // 不可缓存行：进返回列表但不落缓存、不进 keep（若缓存里有同 key 旧行
        // 会走 deletes 删除），保证下轮继续重新解析而非命中过期行。
        if !cacheable(&meta) {
            uncacheable += 1;
            sessions.push(meta);
            continue;
        }
        let Ok(meta_json) = serde_json::to_string(&meta) else {
            continue;
        };
        keep.insert(key.clone());
        upserts.push(SessionScanCacheEntry {
            file_path: key,
            provider: provider.to_string(),
            mtime_ns: target.mtime_ns,
            size: target.size,
            meta_json,
            cache_version: SCAN_CACHE_VERSION,
        });
        sessions.push(meta);
    }

    let deletes = cached
        .into_keys()
        .filter(|key| !keep.contains(key))
        .collect();

    ScanDelta {
        sessions,
        upserts,
        deletes,
        uncacheable,
    }
}

/// Parse `targets` into metadata, preserving input order and pairing each result
/// with its target. Small inputs run serially; larger ones fan out with the same
/// conservative worker cap as [`super::providers::utils::parse_sessions_parallel`]
/// so the background scan never starves the single-threaded UI loop.
fn parse_targets_parallel<F>(targets: &[FileScanTarget], parse: &F) -> Vec<Option<SessionMeta>>
where
    F: Fn(&Path) -> Option<SessionMeta> + Sync,
{
    let workers = std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(2)
        .min(4);
    if workers <= 1 || targets.len() < 64 {
        return targets.iter().map(|t| parse(&t.path)).collect();
    }
    let chunk_size = targets.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let handles: Vec<_> = targets
            .chunks(chunk_size)
            .map(|chunk| scope.spawn(|| chunk.iter().map(|t| parse(&t.path)).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| {
                // 结果必须与 targets 逐位对齐（调用方按 zip 配对路径与解析结果），
                // 吞掉 panic 会让结果变短、错位写坏缓存；这里选择向上传播，由
                // session worker 的 catch_unwind 统一降级为扫描失败。
                handle.join().expect("session parse worker panicked")
            })
            .collect()
    })
}

/// Run the cache-aware scan for one file-parse-backed provider: load the cached
/// rows, reconcile them against the current files, persist the delta, and return
/// the (unsorted) session list. Store errors degrade gracefully — a failed load
/// behaves like an empty cache (full parse) and failed writes are logged — so a
/// cache hiccup never breaks scanning.
pub fn scan_provider_cached<F, C>(
    store: &ScanCacheStore,
    provider: &str,
    targets: Vec<FileScanTarget>,
    force: bool,
    parse: F,
    cacheable: C,
) -> Vec<SessionMeta>
where
    F: Fn(&Path) -> Option<SessionMeta> + Sync,
    C: Fn(&SessionMeta) -> bool + Sync,
{
    let started = std::time::Instant::now();
    let cached = store.load_for_provider(provider).unwrap_or_else(|err| {
        log::warn!("session scan cache load failed for {provider}: {err}");
        HashMap::new()
    });

    let target_count = targets.len();
    let cached_count = cached.len();
    let delta = revalidate(provider, targets, cached, force, parse, cacheable);
    log::debug!(
        "[SESSION-SCAN] provider={provider} targets={target_count} cached={cached_count} \
         reparsed={} deleted={} uncacheable={} force={force} elapsed={:?}",
        delta.upserts.len(),
        delta.deletes.len(),
        delta.uncacheable,
        started.elapsed()
    );

    if !delta.upserts.is_empty() {
        if let Err(err) = store.upsert_batch(&delta.upserts) {
            log::warn!("session scan cache upsert failed for {provider}: {err}");
        }
    }
    if !delta.deletes.is_empty() {
        if let Err(err) = store.delete_paths(&delta.deletes) {
            log::warn!("session scan cache delete failed for {provider}: {err}");
        }
    }

    delta.sessions
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A parse closure that records how often it runs and derives the session id
    /// from the file's first line, so a cache hit (no re-read) is observable both
    /// by the counter staying flat and by the returned id being the stale one.
    fn counting_parse<'a>(
        counter: &'a AtomicUsize,
    ) -> impl Fn(&Path) -> Option<SessionMeta> + Sync + 'a {
        move |path: &Path| {
            counter.fetch_add(1, Ordering::SeqCst);
            let content = std::fs::read_to_string(path).ok()?;
            let id = content.lines().next()?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            let mut meta = sample_meta(&id);
            meta.source_path = Some(path.to_string_lossy().to_string());
            Some(meta)
        }
    }

    #[test]
    fn cache_lifecycle_seed_reuse_change_and_delete() {
        let store = ScanCacheStore::in_memory().expect("memory store");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "id-001\n").expect("write");

        let targets = || {
            let mut out = Vec::new();
            collect_targets_recursive(dir.path(), "jsonl", &mut out);
            out
        };
        let counter = AtomicUsize::new(0);

        // 1. First scan seeds the cache and parses the one file.
        let first = scan_provider_cached(
            &store,
            "claude",
            targets(),
            false,
            counting_parse(&counter),
            |_| true,
        );
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].session_id, "id-001");

        // 2. Rewrite the content to a different value of the SAME byte length and
        //    restore the mtime, so `(mtime_ns, size)` is unchanged. The second scan
        //    must NOT re-read the file: the counter stays flat and the returned id
        //    is the stale cached "id-001", proving the corrupted content was ignored.
        let original_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, "id-XXX\n").expect("rewrite same length");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(original_mtime)
            .expect("restore mtime");

        let second = scan_provider_cached(
            &store,
            "claude",
            targets(),
            false,
            counting_parse(&counter),
            |_| true,
        );
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "unchanged file must not re-parse"
        );
        assert_eq!(second[0].session_id, "id-001");

        // 3. Append (changes size, so the file re-parses) → picks up new content.
        {
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(f, "extra").unwrap();
        }
        // The first line is still "id-XXX" from step 2's rewrite.
        let third = scan_provider_cached(
            &store,
            "claude",
            targets(),
            false,
            counting_parse(&counter),
            |_| true,
        );
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "changed file must re-parse"
        );
        assert_eq!(third[0].session_id, "id-XXX");

        // 4. Delete the file → its cache row is removed and the list is empty.
        std::fs::remove_file(&path).expect("delete");
        let fourth = scan_provider_cached(
            &store,
            "claude",
            targets(),
            false,
            counting_parse(&counter),
            |_| true,
        );
        assert!(fourth.is_empty());
        assert!(store.load_for_provider("claude").expect("load").is_empty());
    }

    #[test]
    fn force_reload_reparses_even_when_unchanged() {
        let store = ScanCacheStore::in_memory().expect("memory store");
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("s.jsonl"), "id-1\n").expect("write");

        let targets = || {
            let mut out = Vec::new();
            collect_targets_recursive(dir.path(), "jsonl", &mut out);
            out
        };
        let counter = AtomicUsize::new(0);

        scan_provider_cached(
            &store,
            "claude",
            targets(),
            false,
            counting_parse(&counter),
            |_| true,
        );
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        // A forced reload re-parses the unchanged file (mtime/size ignored).
        scan_provider_cached(
            &store,
            "claude",
            targets(),
            true,
            counting_parse(&counter),
            |_| true,
        );
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    fn sample_meta(session_id: &str) -> SessionMeta {
        SessionMeta {
            provider_id: "claude".to_string(),
            session_id: session_id.to_string(),
            title: Some("title".to_string()),
            summary: Some("summary".to_string()),
            project_dir: Some("/tmp/project".to_string()),
            created_at: Some(1_000),
            last_active_at: Some(2_000),
            source_path: Some(format!("/tmp/{session_id}.jsonl")),
            resume_command: Some(format!("claude --resume {session_id}")),
        }
    }

    fn cached_row(target: &FileScanTarget, meta: &SessionMeta, version: i64) -> CachedScanRow {
        CachedScanRow {
            mtime_ns: target.mtime_ns,
            size: target.size,
            cache_version: version,
            meta_json: serde_json::to_string(meta).unwrap(),
        }
    }

    #[test]
    fn session_meta_json_roundtrip_is_identity() {
        let meta = sample_meta("abc");
        let json = serde_json::to_string(&meta).expect("serialize");
        let back: SessionMeta = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(meta, back);
    }

    #[test]
    fn session_meta_deserialize_tolerates_missing_fields() {
        // A row written by an older build that only stored the two required
        // fields must still deserialize, with the rest defaulted.
        let meta: SessionMeta =
            serde_json::from_str(r#"{"providerId":"claude","sessionId":"abc"}"#).expect("parse");
        assert_eq!(meta.session_id, "abc");
        assert_eq!(meta.title, None);
        assert_eq!(meta.created_at, None);
    }

    #[test]
    fn revalidate_reuses_unchanged_and_reparses_changed() {
        let unchanged = FileScanTarget {
            path: PathBuf::from("/tmp/a.jsonl"),
            mtime_ns: 100,
            size: 10,
        };
        let changed = FileScanTarget {
            path: PathBuf::from("/tmp/b.jsonl"),
            mtime_ns: 200,
            size: 20,
        };
        let mut cached = HashMap::new();
        cached.insert(
            "/tmp/a.jsonl".to_string(),
            cached_row(&unchanged, &sample_meta("a"), SCAN_CACHE_VERSION),
        );
        // Stored size differs from the current file, so `b` must be re-parsed.
        cached.insert(
            "/tmp/b.jsonl".to_string(),
            CachedScanRow {
                size: 999,
                ..cached_row(&changed, &sample_meta("b-old"), SCAN_CACHE_VERSION)
            },
        );

        let parsed = std::sync::atomic::AtomicUsize::new(0);
        let delta = revalidate(
            "claude",
            vec![unchanged, changed],
            cached,
            false,
            |path| {
                parsed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some(sample_meta(path.file_stem().unwrap().to_str().unwrap()))
            },
            |_| true,
        );

        // Only the changed file is parsed; the unchanged one is a cache hit.
        assert_eq!(parsed.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(delta.upserts.len(), 1);
        assert_eq!(delta.upserts[0].file_path, "/tmp/b.jsonl");
        assert!(delta.deletes.is_empty());
        assert_eq!(delta.sessions.len(), 2);
    }

    #[test]
    fn revalidate_deletes_vanished_files() {
        let present = FileScanTarget {
            path: PathBuf::from("/tmp/a.jsonl"),
            mtime_ns: 100,
            size: 10,
        };
        let mut cached = HashMap::new();
        cached.insert(
            "/tmp/a.jsonl".to_string(),
            cached_row(&present, &sample_meta("a"), SCAN_CACHE_VERSION),
        );
        cached.insert(
            "/tmp/gone.jsonl".to_string(),
            cached_row(&present, &sample_meta("gone"), SCAN_CACHE_VERSION),
        );

        let delta = revalidate(
            "claude",
            vec![present],
            cached,
            false,
            |_| Some(sample_meta("a")),
            |_| true,
        );

        assert_eq!(delta.deletes, vec!["/tmp/gone.jsonl".to_string()]);
        assert_eq!(delta.sessions.len(), 1);
    }

    #[test]
    fn revalidate_reparses_when_cache_version_mismatches() {
        let target = FileScanTarget {
            path: PathBuf::from("/tmp/a.jsonl"),
            mtime_ns: 100,
            size: 10,
        };
        let mut cached = HashMap::new();
        // Same mtime/size but a stale cache version → must re-parse.
        cached.insert(
            "/tmp/a.jsonl".to_string(),
            cached_row(&target, &sample_meta("a"), SCAN_CACHE_VERSION - 1),
        );

        let parsed = std::sync::atomic::AtomicUsize::new(0);
        let delta = revalidate(
            "claude",
            vec![target],
            cached,
            false,
            |_| {
                parsed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some(sample_meta("a"))
            },
            |_| true,
        );

        assert_eq!(parsed.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(delta.upserts.len(), 1);
        assert_eq!(delta.upserts[0].cache_version, SCAN_CACHE_VERSION);
        assert!(delta.deletes.is_empty());
    }

    #[test]
    fn revalidate_force_reparses_everything() {
        let target = FileScanTarget {
            path: PathBuf::from("/tmp/a.jsonl"),
            mtime_ns: 100,
            size: 10,
        };
        let mut cached = HashMap::new();
        cached.insert(
            "/tmp/a.jsonl".to_string(),
            cached_row(&target, &sample_meta("a"), SCAN_CACHE_VERSION),
        );

        let parsed = std::sync::atomic::AtomicUsize::new(0);
        let delta = revalidate(
            "claude",
            vec![target],
            cached,
            true,
            |_| {
                parsed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some(sample_meta("a"))
            },
            |_| true,
        );

        // Even an mtime/size match is ignored under `force`.
        assert_eq!(parsed.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(delta.upserts.len(), 1);
    }

    /// cacheable=false 的行每轮都重新解析且不落缓存（OpenCode 无 title 会话
    /// 语义）：文件未变，counter 仍逐轮递增，缓存始终为空。
    #[test]
    fn uncacheable_rows_reparse_every_round_and_are_not_cached() {
        let store = ScanCacheStore::in_memory().expect("memory store");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "id-001\n").expect("write");

        let targets = || {
            let mut out = Vec::new();
            collect_targets_recursive(dir.path(), "jsonl", &mut out);
            out
        };
        let counter = AtomicUsize::new(0);

        let first = scan_provider_cached(
            &store,
            "opencode",
            targets(),
            false,
            counting_parse(&counter),
            |_| false,
        );
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert_eq!(first.len(), 1);
        assert!(
            store
                .load_for_provider("opencode")
                .expect("load")
                .is_empty(),
            "不可缓存行不落缓存"
        );

        // 文件完全未变，但仍每轮重新解析（无命中捷径），缓存依旧为空。
        let second = scan_provider_cached(
            &store,
            "opencode",
            targets(),
            false,
            counting_parse(&counter),
            |_| false,
        );
        assert_eq!(counter.load(Ordering::SeqCst), 2, "未变文件仍每轮重新解析");
        assert_eq!(second.len(), 1);
        assert!(store
            .load_for_provider("opencode")
            .expect("load")
            .is_empty());
    }

    /// 升级前遗留的缓存行本轮判定为不可缓存：不走命中捷径 → 重新解析、不
    /// upsert，且旧行进 deletes 被清除，避免命中过期行。
    #[test]
    fn revalidate_deletes_stale_row_when_now_uncacheable() {
        let target = FileScanTarget {
            path: PathBuf::from("/tmp/a.jsonl"),
            mtime_ns: 100,
            size: 10,
        };
        // (mtime,size) 与当前 target 完全吻合的遗留缓存行。
        let mut cached = HashMap::new();
        cached.insert(
            "/tmp/a.jsonl".to_string(),
            cached_row(&target, &sample_meta("a"), SCAN_CACHE_VERSION),
        );

        let parsed = AtomicUsize::new(0);
        let delta = revalidate(
            "opencode",
            vec![target],
            cached,
            false,
            |_| {
                parsed.fetch_add(1, Ordering::SeqCst);
                Some(sample_meta("a"))
            },
            |_| false,
        );

        assert_eq!(
            parsed.load(Ordering::SeqCst),
            1,
            "命中捷径被禁用 → 重新解析"
        );
        assert!(delta.upserts.is_empty(), "不可缓存行不落缓存");
        assert_eq!(
            delta.deletes,
            vec!["/tmp/a.jsonl".to_string()],
            "旧行被删除以便下轮重新解析"
        );
        assert_eq!(delta.sessions.len(), 1, "仍进返回列表");
        assert_eq!(delta.uncacheable, 1, "观测计数");
    }
}
