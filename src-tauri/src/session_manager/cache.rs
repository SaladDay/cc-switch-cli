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
use std::ops::ControlFlow;
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

/// Hard upper bound for one streaming reconciliation batch. Discovery, cache
/// lookup, parse results, and cache writes are all released before the next
/// batch is accepted.
pub(crate) const STREAM_SCAN_BATCH_SIZE: usize = 128;

/// Why a bounded provider stream ended before authoritative EOF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamScanStop {
    Cancelled,
    SinkStopped,
    /// The source could not be enumerated or decoded authoritatively. Callers
    /// must discard the staging generation and retain the last published one.
    Incomplete,
}

pub(crate) trait IntoStreamParseResult {
    fn into_stream_parse_result(self) -> Result<Option<SessionMeta>, StreamScanStop>;
}

impl IntoStreamParseResult for Option<SessionMeta> {
    fn into_stream_parse_result(self) -> Result<Option<SessionMeta>, StreamScanStop> {
        Ok(self)
    }
}

impl IntoStreamParseResult for Result<Option<SessionMeta>, StreamScanStop> {
    fn into_stream_parse_result(self) -> Result<Option<SessionMeta>, StreamScanStop> {
        self
    }
}

/// Bounded observability counters. None of these counters retain row data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StreamScanStats {
    pub discovered: usize,
    pub emitted: usize,
    pub cache_hits: usize,
    pub reparsed: usize,
    pub uncacheable: usize,
    pub stale_cache_deleted: usize,
    pub max_batch_targets: usize,
}

impl StreamScanStats {
    pub(crate) fn merge(&mut self, other: Self) {
        self.discovered = self.discovered.saturating_add(other.discovered);
        self.emitted = self.emitted.saturating_add(other.emitted);
        self.cache_hits = self.cache_hits.saturating_add(other.cache_hits);
        self.reparsed = self.reparsed.saturating_add(other.reparsed);
        self.uncacheable = self.uncacheable.saturating_add(other.uncacheable);
        self.stale_cache_deleted = self
            .stale_cache_deleted
            .saturating_add(other.stale_cache_deleted);
        self.max_batch_targets = self.max_batch_targets.max(other.max_batch_targets);
    }
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
/// path is missing, is not a regular file, or cannot be inspected.
pub fn stat_target(path: &Path) -> Option<FileScanTarget> {
    stat_target_strict(path).ok().flatten()
}

/// Strict counterpart used by authoritative streams. A concurrent removal is
/// represented as `Ok(None)`; permission and other I/O failures remain errors
/// so callers cannot accidentally publish a partial generation.
pub(crate) fn stat_target_strict(path: &Path) -> std::io::Result<Option<FileScanTarget>> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !meta.is_file() {
        return Ok(None);
    }
    let mtime_ns = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    Ok(Some(FileScanTarget {
        path: path.to_path_buf(),
        mtime_ns,
        size: meta.len() as i64,
    }))
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
    let _ = mix_sibling_into_fingerprint_strict(target, sibling);
}

/// Strict sibling fingerprinting for authoritative streams. A missing optional
/// sibling is valid; other metadata failures mean the derived SessionMeta could
/// not be observed consistently.
pub(crate) fn mix_sibling_into_fingerprint_strict(
    target: &mut FileScanTarget,
    sibling: &Path,
) -> std::io::Result<()> {
    let meta = match std::fs::metadata(sibling) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let sibling_mtime = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);
    target.mtime_ns = target.mtime_ns.max(sibling_mtime);
    target.size = target.size.wrapping_add(meta.len() as i64);
    Ok(())
}

/// Recursively collect files whose extension equals `ext`, statting each once.
/// Mirrors the directory walks the file scanners already use, but reads only
/// metadata (readdir + stat) rather than opening file contents.
#[allow(dead_code)]
pub fn collect_targets_recursive(root: &Path, ext: &str, out: &mut Vec<FileScanTarget>) {
    let _ = collect_targets_recursive_cancellable(root, ext, out, &|| false);
}

/// Cancellation-aware form of [`collect_targets_recursive`]. Returns `false`
/// when traversal stopped early; callers must discard the partial target list.
pub(crate) fn collect_targets_recursive_cancellable(
    root: &Path,
    ext: &str,
    out: &mut Vec<FileScanTarget>,
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
            if !collect_targets_recursive_cancellable(&path, ext, out, is_cancelled) {
                return false;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            if let Some(target) = stat_target(&path) {
                out.push(target);
            }
        }
    }
    true
}

/// Collect files **directly inside** `dir` whose extension equals `ext`, statting
/// each once. Unlike [`collect_targets_recursive`] this does **not** descend into
/// subdirectories — it mirrors the single-level `read_dir` walk that some
/// providers' `scan_sessions` use (Gemini `chats/*.json`, OpenClaw
/// `sessions/*.jsonl`), so the cache path collects exactly the same files the
/// legacy path shows. `stat_target` excludes non-regular files, so a directory
/// whose name happens to end in `.ext` is skipped just like the legacy walk.
#[allow(dead_code)]
pub fn collect_targets_flat(dir: &Path, ext: &str, out: &mut Vec<FileScanTarget>) {
    let _ = collect_targets_flat_cancellable(dir, ext, out, &|| false);
}

/// Cancellation-aware form of [`collect_targets_flat`]. Returns `false` when
/// the partial result must be discarded.
pub(crate) fn collect_targets_flat_cancellable(
    dir: &Path,
    ext: &str,
    out: &mut Vec<FileScanTarget>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> bool {
    if is_cancelled() {
        return false;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return true,
    };
    for entry in entries.flatten() {
        if is_cancelled() {
            return false;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(ext) {
            continue;
        }
        if let Some(target) = stat_target(&path) {
            out.push(target);
        }
    }
    true
}

/// Visit matching files one at a time without first collecting the directory
/// tree. The only traversal state is the recursive directory stack; each target
/// is statted immediately before being handed to the caller.
pub(crate) fn visit_targets_recursive_cancellable(
    root: &Path,
    ext: &str,
    on_target: &mut dyn FnMut(FileScanTarget) -> Result<(), StreamScanStop>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), StreamScanStop> {
    visit_targets_recursive_inner(root, ext, on_target, is_cancelled, true)
}

fn visit_targets_recursive_inner(
    root: &Path,
    ext: &str,
    on_target: &mut dyn FnMut(FileScanTarget) -> Result<(), StreamScanStop>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    logical_root: bool,
) -> Result<(), StreamScanStop> {
    if is_cancelled() {
        return Err(StreamScanStop::Cancelled);
    }
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if logical_root && error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            log::warn!(
                "authoritative session walk failed at {}: {error}",
                root.display()
            );
            return Err(StreamScanStop::Incomplete);
        }
    };
    for entry in entries {
        if is_cancelled() {
            return Err(StreamScanStop::Cancelled);
        }
        let entry = entry.map_err(|error| {
            log::warn!(
                "authoritative session directory entry failed at {}: {error}",
                root.display()
            );
            StreamScanStop::Incomplete
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            log::warn!(
                "authoritative session file type failed at {}: {error}",
                path.display()
            );
            StreamScanStop::Incomplete
        })?;
        if file_type.is_dir() {
            visit_targets_recursive_inner(&path, ext, on_target, is_cancelled, false)?;
            continue;
        }

        // Follow regular-file symlinks for compatibility with the legacy
        // scanners. Directory symlinks are deliberately not followed: provider
        // roots are already followed by `read_dir`, while nested links could
        // create cycles and would require provider-wide visited state.
        let is_candidate = file_type.is_file()
            || (file_type.is_symlink()
                && std::fs::metadata(&path)
                    .map(|metadata| metadata.is_file())
                    .map_err(|error| {
                        log::warn!(
                            "authoritative session symlink stat failed at {}: {error}",
                            path.display()
                        );
                        StreamScanStop::Incomplete
                    })?);
        if !is_candidate || path.extension().and_then(|value| value.to_str()) != Some(ext) {
            continue;
        }
        let target = stat_target_strict(&path)
            .map_err(|error| {
                log::warn!(
                    "authoritative session stat failed at {}: {error}",
                    path.display()
                );
                StreamScanStop::Incomplete
            })?
            .ok_or(StreamScanStop::Incomplete)?;
        on_target(target)?;
    }
    Ok(())
}

fn visit_target_entry_flat(
    entry: std::fs::DirEntry,
    ext: &str,
    on_target: &mut dyn FnMut(FileScanTarget) -> Result<(), StreamScanStop>,
) -> Result<(), StreamScanStop> {
    let path = entry.path();
    if path.extension().and_then(|value| value.to_str()) != Some(ext) {
        return Ok(());
    }
    let file_type = entry.file_type().map_err(|error| {
        log::warn!(
            "authoritative session file type failed at {}: {error}",
            path.display()
        );
        StreamScanStop::Incomplete
    })?;
    let is_candidate = file_type.is_file()
        || (file_type.is_symlink()
            && std::fs::metadata(&path)
                .map(|metadata| metadata.is_file())
                .map_err(|error| {
                    log::warn!(
                        "authoritative session symlink stat failed at {}: {error}",
                        path.display()
                    );
                    StreamScanStop::Incomplete
                })?);
    if is_candidate {
        let target = stat_target_strict(&path)
            .map_err(|error| {
                log::warn!(
                    "authoritative session stat failed at {}: {error}",
                    path.display()
                );
                StreamScanStop::Incomplete
            })?
            .ok_or(StreamScanStop::Incomplete)?;
        on_target(target)?;
    }
    Ok(())
}

/// Flat counterpart to [`visit_targets_recursive_cancellable`].
pub(crate) fn visit_targets_flat_cancellable(
    dir: &Path,
    ext: &str,
    on_target: &mut dyn FnMut(FileScanTarget) -> Result<(), StreamScanStop>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), StreamScanStop> {
    if is_cancelled() {
        return Err(StreamScanStop::Cancelled);
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            log::warn!(
                "authoritative session walk failed at {}: {error}",
                dir.display()
            );
            return Err(StreamScanStop::Incomplete);
        }
    };
    for entry in entries {
        if is_cancelled() {
            return Err(StreamScanStop::Cancelled);
        }
        let entry = entry.map_err(|error| {
            log::warn!(
                "authoritative session directory entry failed at {}: {error}",
                dir.display()
            );
            StreamScanStop::Incomplete
        })?;
        visit_target_entry_flat(entry, ext, on_target)?;
    }
    Ok(())
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
///
/// `restat` re-`stat`s a target's path just before its parse result is written
/// (fix 3). Production passes [`stat_target`]; only when the re-stat still returns
/// the *same* `(mtime_ns, size)` as the pre-parse target is the row upserted. This
/// closes a delete/rewrite race: a TUI scan thread that read a session the user
/// then deleted (its sidecar row already purged) must not upsert the stale row
/// back — otherwise a later restart's stale first-paint snapshot resurrects the
/// deleted session. A re-stat that vanished or changed skips the upsert and, by
/// not entering `keep`, lets any pre-existing cache row for that path be deleted;
/// the file is reprocessed on the next scan.
#[allow(dead_code)]
pub fn revalidate<F, C, R>(
    provider: &str,
    targets: Vec<FileScanTarget>,
    cached: HashMap<String, CachedScanRow>,
    force: bool,
    parse: F,
    cacheable: C,
    restat: R,
) -> ScanDelta
where
    F: Fn(&Path) -> Option<SessionMeta> + Sync,
    C: Fn(&SessionMeta) -> bool + Sync,
    R: Fn(&Path) -> Option<FileScanTarget>,
{
    revalidate_progressive(
        provider,
        targets,
        cached,
        force,
        parse,
        cacheable,
        restat,
        |_| {},
    )
}

/// Progressive variant of [`revalidate`]. `on_session` runs on the calling
/// scan thread as soon as each cache hit or parsed session becomes available;
/// it is never called from parser worker threads. The returned delta remains
/// complete and preserves the same authoritative ordering/semantics as
/// [`revalidate`].
#[expect(
    clippy::too_many_arguments,
    reason = "cache reconciliation injects parsing, cacheability, restat, and progress policies"
)]
pub fn revalidate_progressive<F, C, R, P>(
    provider: &str,
    targets: Vec<FileScanTarget>,
    cached: HashMap<String, CachedScanRow>,
    force: bool,
    parse: F,
    cacheable: C,
    restat: R,
    on_session: P,
) -> ScanDelta
where
    F: Fn(&Path) -> Option<SessionMeta> + Sync,
    C: Fn(&SessionMeta) -> bool + Sync,
    R: Fn(&Path) -> Option<FileScanTarget>,
    P: FnMut(&SessionMeta),
{
    revalidate_progressive_cancellable(
        provider,
        targets,
        cached,
        force,
        parse,
        cacheable,
        restat,
        on_session,
        &|| false,
    )
    .expect("non-cancellable session scan cannot be cancelled")
}

/// Cancellation-aware cache reconciliation. A cancelled pass returns `None`
/// and never persists a partial delta, so an obsolete search cannot delete or
/// overwrite sidecar rows discovered by an authoritative scan.
#[expect(
    clippy::too_many_arguments,
    reason = "cache reconciliation injects parsing, cacheability, restat, progress, and cancellation policies"
)]
pub(crate) fn revalidate_progressive_cancellable<F, C, R, P>(
    provider: &str,
    targets: Vec<FileScanTarget>,
    cached: HashMap<String, CachedScanRow>,
    force: bool,
    parse: F,
    cacheable: C,
    restat: R,
    mut on_session: P,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<ScanDelta>
where
    F: Fn(&Path) -> Option<SessionMeta> + Sync,
    C: Fn(&SessionMeta) -> bool + Sync,
    R: Fn(&Path) -> Option<FileScanTarget>,
    P: FnMut(&SessionMeta),
{
    let mut sessions = Vec::new();
    let mut keep: HashSet<String> = HashSet::new();
    let mut to_parse: Vec<FileScanTarget> = Vec::new();

    for target in targets {
        if is_cancelled() {
            return None;
        }
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
                            on_session(&meta);
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
    let parsed =
        parse_targets_parallel_cancellable(&to_parse, &parse, &mut on_session, is_cancelled)?;
    let mut upserts = Vec::new();
    let mut uncacheable = 0usize;
    for (target, meta) in to_parse.into_iter().zip(parsed) {
        if is_cancelled() {
            return None;
        }
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
        // fix 3：upsert 前对该文件再 stat 一次，关闭 parse 期间的删除/改写竞态。
        // 仅当文件仍在、且 (mtime_ns, size) 与 parse 前完全一致时才写缓存；否则
        // 跳过 upsert 且不进 keep（其旧缓存行由末尾 filter 纳入 deletes），下轮
        // 重新处理该文件。这样在途扫描线程不会把已删/已改文件的旧 row 写回
        // sidecar，避免重启后 stale 首屏快照复活已删会话。仍把已解析的 meta 放入
        // 返回列表（本轮 UI 由内存 tombstone 过滤已删会话；此处只管缓存不被污染）。
        match restat(&target.path) {
            Some(fresh) if fresh.mtime_ns == target.mtime_ns && fresh.size == target.size => {
                keep.insert(key.clone());
                upserts.push(SessionScanCacheEntry {
                    file_path: key,
                    provider: provider.to_string(),
                    mtime_ns: target.mtime_ns,
                    size: target.size,
                    meta_json,
                    cache_version: SCAN_CACHE_VERSION,
                });
            }
            _ => {
                log::debug!(
                    "[SESSION-SCAN] provider={provider} re-stat 失配，跳过 upsert（旧缓存行将删除）: {key}"
                );
            }
        }
        sessions.push(meta);
    }

    let mut deletes = Vec::new();
    for key in cached.into_keys() {
        if is_cancelled() {
            return None;
        }
        if !keep.contains(&key) {
            deletes.push(key);
        }
    }

    if is_cancelled() {
        return None;
    }

    Some(ScanDelta {
        sessions,
        upserts,
        deletes,
        uncacheable,
    })
}

/// Parse `targets` into metadata, preserving input order and pairing each result
/// with its target. Small inputs run serially; larger ones fan out with the same
/// conservative worker cap as [`super::providers::utils::parse_sessions_parallel`]
/// so the background scan never starves the single-threaded UI loop.
fn parse_targets_parallel<F, P>(
    targets: &[FileScanTarget],
    parse: &F,
    on_session: &mut P,
) -> Vec<Option<SessionMeta>>
where
    F: Fn(&Path) -> Option<SessionMeta> + Sync,
    P: FnMut(&SessionMeta),
{
    parse_targets_parallel_cancellable(targets, parse, on_session, &|| false)
        .expect("non-cancellable session parse cannot be cancelled")
}

fn parse_targets_parallel_cancellable<F, P>(
    targets: &[FileScanTarget],
    parse: &F,
    on_session: &mut P,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<Option<SessionMeta>>>
where
    F: Fn(&Path) -> Option<SessionMeta> + Sync,
    P: FnMut(&SessionMeta),
{
    let workers = std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(2)
        .min(4);
    if workers <= 1 || targets.len() < 64 {
        let mut parsed = Vec::with_capacity(targets.len());
        for target in targets {
            if is_cancelled() {
                return None;
            }
            let result = parse(&target.path);
            if let Some(meta) = result.as_ref() {
                on_session(meta);
            }
            parsed.push(result);
        }
        return Some(parsed);
    }
    let chunk_size = targets.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let handles: Vec<_> = targets
            .chunks(chunk_size)
            .enumerate()
            .map(|(chunk_index, chunk)| {
                let result_tx = result_tx.clone();
                scope.spawn(move || {
                    let offset = chunk_index * chunk_size;
                    for (within_chunk, target) in chunk.iter().enumerate() {
                        if is_cancelled() {
                            break;
                        }
                        if result_tx
                            .send((offset + within_chunk, parse(&target.path)))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
            })
            .collect();
        drop(result_tx);

        let mut parsed: Vec<Option<Option<SessionMeta>>> = std::iter::repeat_with(|| None)
            .take(targets.len())
            .collect();
        while !is_cancelled() {
            match result_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                Ok((index, result)) => {
                    if let Some(meta) = result.as_ref() {
                        on_session(meta);
                    }
                    parsed[index] = Some(result);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        drop(result_rx);
        for handle in handles {
            // Propagate parser panics to the outer session worker catch_unwind.
            handle.join().expect("session parse worker panicked");
        }
        if is_cancelled() {
            return None;
        }
        Some(
            parsed
                .into_iter()
                .map(|result| result.expect("session parse result index must be filled"))
                .collect(),
        )
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
    scan_provider_cached_progressive(store, provider, targets, force, parse, cacheable, |_| {})
}

/// Progressive version of [`scan_provider_cached`]. The callback is invoked
/// before the full provider scan completes whenever at least one valid session
/// can be produced, including on a first run with an empty sidecar.
pub fn scan_provider_cached_progressive<F, C, P>(
    store: &ScanCacheStore,
    provider: &str,
    targets: Vec<FileScanTarget>,
    force: bool,
    parse: F,
    cacheable: C,
    on_session: P,
) -> Vec<SessionMeta>
where
    F: Fn(&Path) -> Option<SessionMeta> + Sync,
    C: Fn(&SessionMeta) -> bool + Sync,
    P: FnMut(&SessionMeta),
{
    let started = std::time::Instant::now();
    let cached = store.load_for_provider(provider).unwrap_or_else(|err| {
        log::warn!("session scan cache load failed for {provider}: {err}");
        HashMap::new()
    });

    let target_count = targets.len();
    let cached_count = cached.len();
    // fix 3：生产侧用真实 stat_target 做 upsert 前的 re-stat（关闭 parse 期间竞态）。
    let delta = revalidate_progressive(
        provider,
        targets,
        cached,
        force,
        parse,
        cacheable,
        stat_target,
        on_session,
    );
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

/// Search-only variant of [`scan_provider_cached_progressive`]. Cancellation
/// discards the partial reconciliation before any cache write.
#[expect(
    clippy::too_many_arguments,
    reason = "provider scan receives parser, cacheability, progress, and cancellation policies"
)]
pub(crate) fn scan_provider_cached_progressive_cancellable<F, C, P>(
    store: &ScanCacheStore,
    provider: &str,
    targets: Vec<FileScanTarget>,
    force: bool,
    parse: F,
    cacheable: C,
    on_session: P,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionMeta>>
where
    F: Fn(&Path) -> Option<SessionMeta> + Sync,
    C: Fn(&SessionMeta) -> bool + Sync,
    P: FnMut(&SessionMeta),
{
    let started = std::time::Instant::now();
    let cached = match store.load_for_provider_cancellable(provider, is_cancelled) {
        Ok(Some(cached)) => cached,
        Ok(None) => return None,
        Err(err) => {
            log::warn!("session scan cache load failed for {provider}: {err}");
            HashMap::new()
        }
    };

    let target_count = targets.len();
    let cached_count = cached.len();
    let delta = revalidate_progressive_cancellable(
        provider,
        targets,
        cached,
        force,
        parse,
        cacheable,
        stat_target,
        on_session,
        is_cancelled,
    )?;
    if is_cancelled() {
        return None;
    }
    log::debug!(
        "[SESSION-SEARCH-SCAN] provider={provider} targets={target_count} cached={cached_count} \
         reparsed={} deleted={} uncacheable={} force={force} elapsed={:?}",
        delta.upserts.len(),
        delta.deletes.len(),
        delta.uncacheable,
        started.elapsed()
    );

    if is_cancelled() {
        None
    } else {
        Some(delta.sessions)
    }
}

/// Progressive uncached scan used when the local sidecar cannot be opened.
/// Parsing still streams completed rows to the worker while retaining every
/// result for the final authoritative Vec.
pub fn scan_provider_uncached_progressive<F, P>(
    targets: Vec<FileScanTarget>,
    parse: F,
    mut on_session: P,
) -> Vec<SessionMeta>
where
    F: Fn(&Path) -> Option<SessionMeta> + Sync,
    P: FnMut(&SessionMeta),
{
    parse_targets_parallel(&targets, &parse, &mut on_session)
        .into_iter()
        .flatten()
        .collect()
}

pub(crate) fn scan_provider_uncached_progressive_cancellable<F, P>(
    targets: Vec<FileScanTarget>,
    parse: F,
    mut on_session: P,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionMeta>>
where
    F: Fn(&Path) -> Option<SessionMeta> + Sync,
    P: FnMut(&SessionMeta),
{
    Some(
        parse_targets_parallel_cancellable(&targets, &parse, &mut on_session, is_cancelled)?
            .into_iter()
            .flatten()
            .collect(),
    )
}

/// Stream a file-backed provider into an owned-row sink without ever building
/// either a full target list, a provider-wide cache map, or a provider result
/// `Vec`. `walk` must call the supplied target callback as paths are discovered.
///
/// Cache failures degrade to reparsing the affected fixed-size batch. A
/// completed traversal also removes cache rows whose source file disappeared,
/// using a keyset cursor in [`ScanCacheStore`] rather than an O(N) seen set.
#[expect(
    clippy::too_many_arguments,
    reason = "streaming reconciliation injects walker, parser, fingerprint, cacheability, sink, and cancellation policies"
)]
pub(crate) fn stream_file_provider_cancellable<F, O, C, R, W>(
    store: Option<&ScanCacheStore>,
    provider: &str,
    force: bool,
    parse: F,
    cacheable: C,
    restat: R,
    walk: W,
    on_session: &mut dyn FnMut(SessionMeta) -> ControlFlow<()>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<StreamScanStats, StreamScanStop>
where
    F: Fn(&Path) -> O + Sync,
    O: IntoStreamParseResult,
    C: Fn(&SessionMeta) -> bool + Sync,
    R: Fn(&Path) -> Option<FileScanTarget>,
    W: FnOnce(
        &mut dyn FnMut(FileScanTarget) -> Result<(), StreamScanStop>,
        &(dyn Fn() -> bool + Sync),
    ) -> Result<(), StreamScanStop>,
{
    if is_cancelled() {
        return Err(StreamScanStop::Cancelled);
    }

    let started = std::time::Instant::now();
    let mut stats = StreamScanStats::default();
    let mut batch = Vec::with_capacity(STREAM_SCAN_BATCH_SIZE);
    let mut accept_target = |target: FileScanTarget| {
        if is_cancelled() {
            return Err(StreamScanStop::Cancelled);
        }
        stats.discovered = stats.discovered.saturating_add(1);
        batch.push(target);
        stats.max_batch_targets = stats.max_batch_targets.max(batch.len());
        if batch.len() == STREAM_SCAN_BATCH_SIZE {
            process_stream_batch(
                store,
                provider,
                force,
                &parse,
                &cacheable,
                &restat,
                &mut batch,
                on_session,
                is_cancelled,
                &mut stats,
            )?;
        }
        Ok(())
    };

    walk(&mut accept_target, is_cancelled)?;
    drop(accept_target);
    process_stream_batch(
        store,
        provider,
        force,
        &parse,
        &cacheable,
        &restat,
        &mut batch,
        on_session,
        is_cancelled,
        &mut stats,
    )?;
    if is_cancelled() {
        return Err(StreamScanStop::Cancelled);
    }

    if let Some(store) = store {
        match store.delete_missing_for_provider_bounded(
            provider,
            STREAM_SCAN_BATCH_SIZE,
            is_cancelled,
        ) {
            Ok(Some(deleted)) => {
                stats.stale_cache_deleted = stats.stale_cache_deleted.saturating_add(deleted);
            }
            Ok(None) => return Err(StreamScanStop::Cancelled),
            Err(error) => {
                log::warn!("session scan cache stale cleanup incomplete for {provider}: {error}");
                // The provider walk and every emitted row are already
                // authoritative at this point.  This sidecar is disposable:
                // a stale entry can cost a later `stat`, but it must never
                // prevent a complete source scan from being published.
                if is_cancelled() {
                    return Err(StreamScanStop::Cancelled);
                }
            }
        }
    }
    if is_cancelled() {
        return Err(StreamScanStop::Cancelled);
    }

    log::debug!(
        "[SESSION-STREAM-SCAN] provider={provider} discovered={} emitted={} cache_hits={} \
         reparsed={} uncacheable={} stale_deleted={} max_batch={} force={force} elapsed={:?}",
        stats.discovered,
        stats.emitted,
        stats.cache_hits,
        stats.reparsed,
        stats.uncacheable,
        stats.stale_cache_deleted,
        stats.max_batch_targets,
        started.elapsed()
    );
    Ok(stats)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one bounded batch carries injected cache, parse, fingerprint, sink, and cancellation policies"
)]
fn process_stream_batch<F, O, C, R>(
    store: Option<&ScanCacheStore>,
    provider: &str,
    force: bool,
    parse: &F,
    cacheable: &C,
    restat: &R,
    batch: &mut Vec<FileScanTarget>,
    on_session: &mut dyn FnMut(SessionMeta) -> ControlFlow<()>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    stats: &mut StreamScanStats,
) -> Result<(), StreamScanStop>
where
    F: Fn(&Path) -> O + Sync,
    O: IntoStreamParseResult,
    C: Fn(&SessionMeta) -> bool + Sync,
    R: Fn(&Path) -> Option<FileScanTarget>,
{
    if batch.is_empty() {
        return Ok(());
    }
    if is_cancelled() {
        return Err(StreamScanStop::Cancelled);
    }

    let paths: Vec<String> = batch
        .iter()
        .map(|target| target.path.to_string_lossy().into_owned())
        .collect();
    let cached = match store {
        Some(store) => match store.load_batch_cancellable(provider, &paths, is_cancelled) {
            Ok(Some(rows)) => rows,
            Ok(None) => return Err(StreamScanStop::Cancelled),
            Err(error) => {
                log::warn!("session scan cache batch load failed for {provider}: {error}");
                HashMap::new()
            }
        },
        None => HashMap::new(),
    };

    let mut to_parse = Vec::with_capacity(batch.len());
    for target in batch.drain(..) {
        if is_cancelled() {
            return Err(StreamScanStop::Cancelled);
        }
        let key = target.path.to_string_lossy().into_owned();
        let observed = authoritative_restat(restat, &target.path)?;
        let hit = (!force)
            .then(|| cached.get(&key))
            .flatten()
            .filter(|row| {
                row.cache_version == SCAN_CACHE_VERSION
                    && row.mtime_ns == target.mtime_ns
                    && row.size == target.size
            })
            .and_then(|row| serde_json::from_str::<SessionMeta>(&row.meta_json).ok())
            .filter(|meta| cacheable(meta))
            .filter(|_| {
                observed
                    .as_ref()
                    .is_some_and(|fresh| same_fingerprint(fresh, &target))
            });
        if let Some(meta) = hit {
            if on_session(meta).is_break() {
                return Err(StreamScanStop::SinkStopped);
            }
            stats.cache_hits = stats.cache_hits.saturating_add(1);
            stats.emitted = stats.emitted.saturating_add(1);
        } else {
            to_parse.push(target);
        }
    }

    let mut upserts = Vec::with_capacity(to_parse.len());
    let mut deletes = Vec::new();
    parse_targets_completed_cancellable(&to_parse, parse, is_cancelled, &mut |target, parsed| {
        stats.reparsed = stats.reparsed.saturating_add(1);
        let key = target.path.to_string_lossy().into_owned();
        let observed = authoritative_restat(restat, &target.path)?;
        let stable = observed
            .as_ref()
            .is_some_and(|fresh| same_fingerprint(fresh, target));

        let (settled_target, settled_meta) = if stable {
            (target.clone(), parsed)
        } else {
            let Some(fresh) = observed else {
                // The target existed at discovery but disappeared during parse.
                // Publishing the partial scan would resurrect/lose rows depending
                // on timing, so retain the previous manifest instead.
                return Err(StreamScanStop::Incomplete);
            };
            if let Some(meta) = cached_meta_for_target(&cached, &key, &fresh, cacheable) {
                if on_session(meta).is_break() {
                    return Err(StreamScanStop::SinkStopped);
                }
                stats.cache_hits = stats.cache_hits.saturating_add(1);
                stats.emitted = stats.emitted.saturating_add(1);
                return Ok(());
            }

            // The file changed while it was parsed. Retry exactly once against
            // the latest fingerprint; another change makes the whole generation
            // incomplete rather than emitting a torn SessionMeta.
            if is_cancelled() {
                return Err(StreamScanStop::Cancelled);
            }
            let retry = parse(&fresh.path).into_stream_parse_result()?;
            if is_cancelled() {
                return Err(StreamScanStop::Cancelled);
            }
            stats.reparsed = stats.reparsed.saturating_add(1);
            let after_retry = authoritative_restat(restat, &fresh.path)?;
            if !after_retry
                .as_ref()
                .is_some_and(|after| same_fingerprint(after, &fresh))
            {
                return Err(StreamScanStop::Incomplete);
            }
            (fresh, retry)
        };

        let Some(meta) = settled_meta else {
            // A stable parse-to-None means this file is authoritatively not a
            // session. Only this case may remove an older cache row.
            if cached.contains_key(&key) {
                deletes.push(key);
            }
            return Ok(());
        };

        if cacheable(&meta) {
            match serde_json::to_string(&meta) {
                Ok(meta_json) => upserts.push(SessionScanCacheEntry {
                    file_path: key.clone(),
                    provider: provider.to_string(),
                    mtime_ns: settled_target.mtime_ns,
                    size: settled_target.size,
                    meta_json,
                    cache_version: SCAN_CACHE_VERSION,
                }),
                Err(_) if cached.contains_key(&key) => deletes.push(key.clone()),
                Err(_) => {}
            }
        } else {
            stats.uncacheable = stats.uncacheable.saturating_add(1);
            if cached.contains_key(&key) {
                deletes.push(key);
            }
        }

        if on_session(meta).is_break() {
            return Err(StreamScanStop::SinkStopped);
        }
        stats.emitted = stats.emitted.saturating_add(1);
        Ok(())
    })?;

    if is_cancelled() {
        return Err(StreamScanStop::Cancelled);
    }
    if let Some(store) = store {
        if let Err(error) = store.upsert_batch(&upserts) {
            log::warn!("session scan cache batch upsert failed for {provider}: {error}");
        }
        if let Err(error) = store.delete_paths(&deletes) {
            log::warn!("session scan cache batch delete failed for {provider}: {error}");
        }
    }
    Ok(())
}

fn same_fingerprint(left: &FileScanTarget, right: &FileScanTarget) -> bool {
    left.mtime_ns == right.mtime_ns && left.size == right.size
}

fn authoritative_restat<R>(
    restat: &R,
    path: &Path,
) -> Result<Option<FileScanTarget>, StreamScanStop>
where
    R: Fn(&Path) -> Option<FileScanTarget>,
{
    match stat_target_strict(path) {
        Ok(Some(_)) => Ok(restat(path)),
        Ok(None) => Ok(None),
        Err(error) => {
            log::warn!(
                "authoritative session re-stat failed at {}: {error}",
                path.display()
            );
            Err(StreamScanStop::Incomplete)
        }
    }
}

fn cached_meta_for_target<C>(
    cached: &HashMap<String, CachedScanRow>,
    key: &str,
    target: &FileScanTarget,
    cacheable: &C,
) -> Option<SessionMeta>
where
    C: Fn(&SessionMeta) -> bool + Sync,
{
    let row = cached.get(key)?;
    if row.cache_version != SCAN_CACHE_VERSION
        || row.mtime_ns != target.mtime_ns
        || row.size != target.size
    {
        return None;
    }
    let meta = serde_json::from_str::<SessionMeta>(&row.meta_json).ok()?;
    cacheable(&meta).then_some(meta)
}

/// Parse a fixed batch on a bounded worker pool and deliver results in actual
/// completion order. The sync channel retains at most two results per worker;
/// one slow target therefore cannot hold back unrelated completed metadata.
fn parse_targets_completed_cancellable<F, O, P>(
    targets: &[FileScanTarget],
    parse: &F,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    on_result: &mut P,
) -> Result<(), StreamScanStop>
where
    F: Fn(&Path) -> O + Sync,
    O: IntoStreamParseResult,
    P: FnMut(&FileScanTarget, Option<SessionMeta>) -> Result<(), StreamScanStop>,
{
    if targets.is_empty() {
        return Ok(());
    }
    if is_cancelled() {
        return Err(StreamScanStop::Cancelled);
    }
    let workers = std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(2)
        .min(4)
        .min(targets.len());
    if workers <= 1 {
        for target in targets {
            if is_cancelled() {
                return Err(StreamScanStop::Cancelled);
            }
            on_result(target, parse(&target.path).into_stream_parse_result()?)?;
        }
        return Ok(());
    }

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let stopped = AtomicBool::new(false);
    let (result_tx, result_rx) = std::sync::mpsc::sync_channel(workers * 2);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let result_tx = result_tx.clone();
                let next = &next;
                let stopped = &stopped;
                scope.spawn(move || loop {
                    if stopped.load(Ordering::Acquire) || is_cancelled() {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(target) = targets.get(index) else {
                        break;
                    };
                    let result = parse(&target.path).into_stream_parse_result();
                    if result_tx.send((index, result)).is_err() {
                        break;
                    }
                })
            })
            .collect();
        drop(result_tx);

        let mut received = 0usize;
        let mut outcome = Ok(());
        while received < targets.len() {
            if is_cancelled() {
                outcome = Err(StreamScanStop::Cancelled);
                break;
            }
            match result_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                Ok((index, result)) => {
                    received += 1;
                    let result = match result {
                        Ok(result) => result,
                        Err(stop) => {
                            outcome = Err(stop);
                            break;
                        }
                    };
                    if let Err(stop) = on_result(&targets[index], result) {
                        outcome = Err(stop);
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    if received != targets.len() {
                        outcome = Err(StreamScanStop::Incomplete);
                    }
                    break;
                }
            }
        }
        stopped.store(true, Ordering::Release);
        drop(result_rx);
        for handle in handles {
            handle.join().expect("session parse worker panicked");
        }
        if is_cancelled() {
            Err(StreamScanStop::Cancelled)
        } else {
            outcome
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
    fn cancellable_revalidation_stops_parser_fanout_and_discards_partial_delta() {
        let cancelled = AtomicBool::new(false);
        let parsed = AtomicUsize::new(0);
        let targets: Vec<_> = (0..1_000)
            .map(|index| FileScanTarget {
                path: PathBuf::from(format!("/virtual/session-{index}.jsonl")),
                mtime_ns: 1,
                size: 1,
            })
            .collect();

        let result = revalidate_progressive_cancellable(
            "claude",
            targets,
            HashMap::new(),
            false,
            |_| {
                let count = parsed.fetch_add(1, Ordering::AcqRel) + 1;
                if count >= 4 {
                    cancelled.store(true, Ordering::Release);
                }
                Some(sample_meta(&format!("session-{count}")))
            },
            |_| true,
            |_| None,
            |_| {},
            &|| cancelled.load(Ordering::Acquire),
        );

        assert!(
            result.is_none(),
            "cancelled scan must not expose a partial delta"
        );
        assert!(
            parsed.load(Ordering::Acquire) < 32,
            "parser workers should stop near the cancellation point"
        );
    }

    #[test]
    fn streaming_scan_never_retains_more_than_one_fixed_batch() {
        let dir = tempfile::tempdir().expect("tempdir");
        for index in 0..1_025 {
            std::fs::write(dir.path().join(format!("s-{index:04}.jsonl")), "row").expect("write");
        }
        let parsed = AtomicUsize::new(0);
        let emitted = AtomicUsize::new(0);
        let first_emit_parse_count = AtomicUsize::new(usize::MAX);
        let mut sink = |_: SessionMeta| {
            if emitted.fetch_add(1, Ordering::AcqRel) == 0 {
                first_emit_parse_count.store(parsed.load(Ordering::Acquire), Ordering::Release);
            }
            ControlFlow::Continue(())
        };
        let stats = stream_file_provider_cancellable(
            None,
            "claude",
            false,
            |path| {
                parsed.fetch_add(1, Ordering::AcqRel);
                Some(sample_meta(
                    path.file_stem()
                        .and_then(|value| value.to_str())
                        .expect("stem"),
                ))
            },
            |_| true,
            stat_target,
            |on_target, cancel| {
                visit_targets_recursive_cancellable(dir.path(), "jsonl", on_target, cancel)
            },
            &mut sink,
            &|| false,
        )
        .expect("stream");

        assert_eq!(stats.discovered, 1_025);
        assert_eq!(stats.emitted, 1_025);
        assert_eq!(stats.max_batch_targets, STREAM_SCAN_BATCH_SIZE);
        assert!(
            first_emit_parse_count.load(Ordering::Acquire) <= STREAM_SCAN_BATCH_SIZE,
            "the first owned row must be emitted after one batch, not after full discovery"
        );
    }

    #[test]
    fn streaming_scan_observes_sink_stop_and_cancellation() {
        let dir = tempfile::tempdir().expect("tempdir");
        for index in 0..400 {
            std::fs::write(dir.path().join(format!("s-{index:04}.jsonl")), "row").expect("write");
        }

        let emitted = AtomicUsize::new(0);
        let mut stopping_sink = |_: SessionMeta| {
            if emitted.fetch_add(1, Ordering::AcqRel) >= 4 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let stopped = stream_file_provider_cancellable(
            None,
            "claude",
            false,
            |_| Some(sample_meta("row")),
            |_| true,
            stat_target,
            |on_target, cancel| {
                visit_targets_recursive_cancellable(dir.path(), "jsonl", on_target, cancel)
            },
            &mut stopping_sink,
            &|| false,
        );
        assert_eq!(stopped, Err(StreamScanStop::SinkStopped));
        assert_eq!(emitted.load(Ordering::Acquire), 5);

        let cancelled = AtomicBool::new(false);
        let parsed = AtomicUsize::new(0);
        let mut cancelling_sink = |_: SessionMeta| {
            cancelled.store(true, Ordering::Release);
            ControlFlow::Continue(())
        };
        let result = stream_file_provider_cancellable(
            None,
            "claude",
            false,
            |_| {
                parsed.fetch_add(1, Ordering::AcqRel);
                Some(sample_meta("row"))
            },
            |_| true,
            stat_target,
            |on_target, cancel| {
                visit_targets_recursive_cancellable(dir.path(), "jsonl", on_target, cancel)
            },
            &mut cancelling_sink,
            &|| cancelled.load(Ordering::Acquire),
        );
        assert_eq!(result, Err(StreamScanStop::Cancelled));
        assert!(parsed.load(Ordering::Acquire) <= STREAM_SCAN_BATCH_SIZE);
    }

    #[test]
    fn incomplete_walk_never_runs_stale_cache_cleanup() {
        let store = ScanCacheStore::in_memory().expect("store");
        let missing = "/definitely/missing/session.jsonl".to_string();
        store
            .upsert_batch(&[SessionScanCacheEntry {
                file_path: missing.clone(),
                provider: "claude".to_string(),
                mtime_ns: 1,
                size: 1,
                meta_json: serde_json::to_string(&sample_meta("old")).expect("json"),
                cache_version: SCAN_CACHE_VERSION,
            }])
            .expect("seed cache");
        let mut sink = |_: SessionMeta| ControlFlow::Continue(());
        let result = stream_file_provider_cancellable(
            Some(&store),
            "claude",
            false,
            |_| Some(sample_meta("unused")),
            |_| true,
            stat_target,
            |_, _| Err(StreamScanStop::Incomplete),
            &mut sink,
            &|| false,
        );
        assert_eq!(result, Err(StreamScanStop::Incomplete));
        assert!(store
            .load_for_provider("claude")
            .expect("load cache")
            .contains_key(&missing));
    }

    #[test]
    fn completed_stream_publishes_when_disposable_stale_cleanup_fails() {
        let store = ScanCacheStore::in_memory().expect("store");
        let invalid_stale_path = "/invalid/\0/session.jsonl".to_string();
        store
            .upsert_batch(&[SessionScanCacheEntry {
                file_path: invalid_stale_path,
                provider: "claude".to_string(),
                mtime_ns: 1,
                size: 1,
                meta_json: serde_json::to_string(&sample_meta("stale")).expect("json"),
                cache_version: SCAN_CACHE_VERSION,
            }])
            .expect("seed stale cache row");

        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("live.jsonl"), "row").expect("write live source");
        let mut emitted = Vec::new();
        let result = stream_file_provider_cancellable(
            Some(&store),
            "claude",
            false,
            |_| Some(sample_meta("live")),
            |_| true,
            stat_target,
            |on_target, cancel| {
                visit_targets_flat_cancellable(dir.path(), "jsonl", on_target, cancel)
            },
            &mut |row| {
                emitted.push(row.session_id);
                ControlFlow::Continue(())
            },
            &|| false,
        );

        assert!(
            result.is_ok(),
            "sidecar cleanup errors are non-authoritative"
        );
        assert_eq!(emitted, ["live"]);
    }

    #[test]
    fn parsed_target_that_keeps_changing_is_never_emitted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("moving.jsonl");
        std::fs::write(&path, "seed").expect("seed");
        let parses = AtomicUsize::new(0);
        let mut emitted = Vec::new();
        let result = stream_file_provider_cancellable(
            None,
            "claude",
            false,
            |path| {
                parses.fetch_add(1, Ordering::AcqRel);
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(path)
                    .expect("append");
                file.write_all(b"x").expect("mutate during parse");
                Some(sample_meta("moving"))
            },
            |_| true,
            stat_target,
            |on_target, cancel| {
                visit_targets_flat_cancellable(dir.path(), "jsonl", on_target, cancel)
            },
            &mut |meta| {
                emitted.push(meta.session_id);
                ControlFlow::Continue(())
            },
            &|| false,
        );
        assert_eq!(result, Err(StreamScanStop::Incomplete));
        assert_eq!(
            parses.load(Ordering::Acquire),
            2,
            "only one retry is allowed"
        );
        assert!(emitted.is_empty());
    }

    #[test]
    fn parser_results_are_delivered_in_completion_order() {
        if std::thread::available_parallelism().map_or(1, |value| value.get()) < 2 {
            return;
        }
        let targets: Vec<_> = (0..8)
            .map(|index| FileScanTarget {
                path: PathBuf::from(if index == 0 {
                    "slow.jsonl".to_string()
                } else {
                    format!("fast-{index}.jsonl")
                }),
                mtime_ns: 1,
                size: 1,
            })
            .collect();
        let mut completed = Vec::new();
        parse_targets_completed_cancellable(
            &targets,
            &|path| {
                if path.file_name().and_then(|value| value.to_str()) == Some("slow.jsonl") {
                    std::thread::sleep(std::time::Duration::from_millis(120));
                }
                Some(sample_meta(
                    path.file_stem()
                        .and_then(|value| value.to_str())
                        .expect("stem"),
                ))
            },
            &|| false,
            &mut |target, _| {
                completed.push(target.path.clone());
                Ok(())
            },
        )
        .expect("parse batch");
        assert_ne!(
            completed.first().and_then(|path| path.file_name()),
            Some(std::ffi::OsStr::new("slow.jsonl")),
            "one slow target must not head-of-line block completed peers"
        );
    }

    #[cfg(unix)]
    #[test]
    fn authoritative_walker_follows_regular_file_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real.data");
        let linked = dir.path().join("linked.jsonl");
        std::fs::write(&real, "row").expect("write");
        symlink(&real, &linked).expect("symlink");
        let mut paths = Vec::new();
        visit_targets_flat_cancellable(
            dir.path(),
            "jsonl",
            &mut |target| {
                paths.push(target.path);
                Ok(())
            },
            &|| false,
        )
        .expect("walk");
        assert_eq!(paths, vec![linked]);
    }

    #[test]
    fn streaming_scan_uses_bounded_sidecar_batches_and_prunes_missing_rows() {
        let store = ScanCacheStore::in_memory().expect("store");
        let dir = tempfile::tempdir().expect("tempdir");
        for index in 0..300 {
            std::fs::write(dir.path().join(format!("s-{index:03}.jsonl")), "row").expect("write");
        }
        let parse_count = AtomicUsize::new(0);
        let scan = |parse_count: &AtomicUsize| {
            let mut sink = |_: SessionMeta| ControlFlow::Continue(());
            stream_file_provider_cancellable(
                Some(&store),
                "claude",
                false,
                |path| {
                    parse_count.fetch_add(1, Ordering::AcqRel);
                    Some(sample_meta(
                        path.file_stem()
                            .and_then(|value| value.to_str())
                            .expect("stem"),
                    ))
                },
                |_| true,
                stat_target,
                |on_target, cancel| {
                    visit_targets_recursive_cancellable(dir.path(), "jsonl", on_target, cancel)
                },
                &mut sink,
                &|| false,
            )
            .expect("stream")
        };

        let first = scan(&parse_count);
        assert_eq!(first.reparsed, 300);
        assert_eq!(parse_count.load(Ordering::Acquire), 300);
        let second = scan(&parse_count);
        assert_eq!(second.cache_hits, 300);
        assert_eq!(second.reparsed, 0);
        assert_eq!(second.max_batch_targets, STREAM_SCAN_BATCH_SIZE);

        std::fs::remove_file(dir.path().join("s-150.jsonl")).expect("remove");
        let third = scan(&parse_count);
        assert_eq!(third.emitted, 299);
        assert_eq!(third.stale_cache_deleted, 1);
        assert_eq!(store.load_for_provider("claude").expect("cache").len(), 299);
    }

    /// 平铺收集器只取目录直属文件、不递归子目录，且跳过非目标扩展名。
    #[test]
    fn collect_targets_flat_ignores_subdirectories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("archive")).expect("mkdir");
        // 直属目标文件应被收集
        std::fs::write(root.join("a.json"), "{}").expect("write");
        std::fs::write(root.join("b.json"), "{}").expect("write");
        // 非目标扩展名跳过
        std::fs::write(root.join("note.txt"), "x").expect("write");
        // 嵌套子目录里的文件不应被收集
        std::fs::write(root.join("archive").join("c.json"), "{}").expect("write");

        let mut out = Vec::new();
        collect_targets_flat(root, "json", &mut out);

        assert_eq!(out.len(), 2, "只收集直属 .json 文件");
        assert!(out.iter().all(|t| t.path.parent() == Some(root)));
        assert!(!out
            .iter()
            .any(|t| t.path.file_name().and_then(|n| n.to_str()) == Some("c.json")));
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

    /// fix 3 测试辅助：模拟"parse 期间文件未变"的 re-stat——按路径回显传入 targets
    /// 的 `(mtime_ns, size)`。用于以假路径断言 upsert 的既有用例，使 re-stat 恒命中、
    /// 保持既有语义（未知路径回 None，与真实文件消失一致）。
    fn echoing_restat(targets: &[FileScanTarget]) -> impl Fn(&Path) -> Option<FileScanTarget> {
        let map: HashMap<PathBuf, FileScanTarget> = targets
            .iter()
            .map(|t| (t.path.clone(), t.clone()))
            .collect();
        move |p: &Path| map.get(p).cloned()
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
        let all_targets = vec![unchanged, changed];
        let restat = echoing_restat(&all_targets);
        let delta = revalidate(
            "claude",
            all_targets,
            cached,
            false,
            |path| {
                parsed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some(sample_meta(path.file_stem().unwrap().to_str().unwrap()))
            },
            |_| true,
            restat,
        );

        // Only the changed file is parsed; the unchanged one is a cache hit.
        assert_eq!(parsed.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(delta.upserts.len(), 1);
        assert_eq!(delta.upserts[0].file_path, "/tmp/b.jsonl");
        assert!(delta.deletes.is_empty());
        assert_eq!(delta.sessions.len(), 2);
    }

    #[test]
    fn progressive_callback_fires_before_file_provider_scan_completes() {
        let fast = FileScanTarget {
            path: PathBuf::from("/tmp/fast.jsonl"),
            mtime_ns: 1,
            size: 1,
        };
        let slow = FileScanTarget {
            path: PathBuf::from("/tmp/slow.jsonl"),
            mtime_ns: 2,
            size: 1,
        };
        let targets = vec![fast, slow];
        let restat = echoing_restat(&targets);
        let slow_parsed = std::sync::atomic::AtomicBool::new(false);
        let preview_before_slow = std::sync::atomic::AtomicBool::new(false);

        let delta = revalidate_progressive(
            "claude",
            targets,
            HashMap::new(),
            false,
            |path| {
                if path.file_stem().and_then(|value| value.to_str()) == Some("slow") {
                    slow_parsed.store(true, Ordering::SeqCst);
                }
                Some(sample_meta(
                    path.file_stem()
                        .and_then(|value| value.to_str())
                        .expect("test path stem"),
                ))
            },
            |_| true,
            restat,
            |_| {
                if !slow_parsed.load(Ordering::SeqCst) {
                    preview_before_slow.store(true, Ordering::SeqCst);
                }
            },
        );

        assert_eq!(delta.sessions.len(), 2);
        assert!(slow_parsed.load(Ordering::SeqCst));
        assert!(
            preview_before_slow.load(Ordering::SeqCst),
            "the first preview must be observable before the provider's remaining files finish"
        );
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

        let all_targets = vec![present];
        let restat = echoing_restat(&all_targets);
        let delta = revalidate(
            "claude",
            all_targets,
            cached,
            false,
            |_| Some(sample_meta("a")),
            |_| true,
            restat,
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
        let all_targets = vec![target];
        let restat = echoing_restat(&all_targets);
        let delta = revalidate(
            "claude",
            all_targets,
            cached,
            false,
            |_| {
                parsed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some(sample_meta("a"))
            },
            |_| true,
            restat,
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
        let all_targets = vec![target];
        let restat = echoing_restat(&all_targets);
        let delta = revalidate(
            "claude",
            all_targets,
            cached,
            true,
            |_| {
                parsed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some(sample_meta("a"))
            },
            |_| true,
            restat,
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
        let all_targets = vec![target];
        // 不可缓存行在 restat 之前就 continue，restat 不会被调用；传回显版即可。
        let restat = echoing_restat(&all_targets);
        let delta = revalidate(
            "opencode",
            all_targets,
            cached,
            false,
            |_| {
                parsed.fetch_add(1, Ordering::SeqCst);
                Some(sample_meta("a"))
            },
            |_| false,
            restat,
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

    /// fix 3：parse 期间文件被删除（re-stat 返回 None）时，不 upsert 该文件，且其
    /// 遗留缓存行进 deletes——在途扫描线程不会把已删文件的旧 row 写回 sidecar。
    #[test]
    fn revalidate_reparse_then_deleted_skips_upsert_and_deletes_old_row() {
        let target = FileScanTarget {
            path: PathBuf::from("/tmp/gone-mid-parse.jsonl"),
            mtime_ns: 100,
            size: 10,
        };
        let key = "/tmp/gone-mid-parse.jsonl".to_string();
        // (mtime,size) 与当前 target 不同的遗留缓存行 → 该文件进 to_parse。
        let mut cached = HashMap::new();
        cached.insert(
            key.clone(),
            CachedScanRow {
                mtime_ns: 1,
                size: 1,
                cache_version: SCAN_CACHE_VERSION,
                meta_json: serde_json::to_string(&sample_meta("old")).unwrap(),
            },
        );

        let delta = revalidate(
            "claude",
            vec![target],
            cached,
            false,
            // parse 成功（返回 meta），但文件在 re-stat 时已消失。
            |_| Some(sample_meta("new")),
            |_| true,
            // 注入的 re-stat 返回 None，模拟 parse 期间文件被删除的竞态。
            |_| None,
        );

        assert!(delta.upserts.is_empty(), "re-stat 失配 → 不 upsert");
        assert!(
            delta.deletes.contains(&key),
            "遗留缓存行进 deletes（不被在途线程写回）"
        );
        // 已解析的 meta 仍进返回列表（本轮 UI 由内存 tombstone 过滤已删会话）。
        assert_eq!(delta.sessions.len(), 1);
    }
}
