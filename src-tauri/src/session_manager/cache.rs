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

use std::collections::HashMap;
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
    /// Raw mtime of the source itself. Unlike `mtime_ns`, sibling fingerprint
    /// decoration must never alter this cache-consistency evidence.
    pub source_mtime_ns: i64,
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
        source_mtime_ns: mtime_ns,
        mtime_ns,
        size: meta.len() as i64,
    }))
}

fn stable_source_mtime_ns(before: &FileScanTarget, after: Option<&FileScanTarget>) -> Option<i64> {
    after
        .filter(|after| same_fingerprint(before, after))
        .map(|_| before.source_mtime_ns)
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
            .filter(|meta| {
                meta.source_mtime_ns
                    .is_none_or(|mtime| mtime == target.source_mtime_ns)
            })
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

        let (settled_target, settled_meta, settled_source_mtime_ns) = if stable {
            (
                target.clone(),
                parsed,
                stable_source_mtime_ns(target, observed.as_ref()),
            )
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
            let source_mtime_ns = stable_source_mtime_ns(&fresh, after_retry.as_ref());
            (fresh, retry, source_mtime_ns)
        };

        let Some(mut meta) = settled_meta else {
            // A stable parse-to-None means this file is authoritatively not a
            // session. Only this case may remove an older cache row.
            if cached.contains_key(&key) {
                deletes.push(key);
            }
            return Ok(());
        };
        meta.source_mtime_ns = settled_source_mtime_ns;

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
    left.source_mtime_ns == right.source_mtime_ns
        && left.mtime_ns == right.mtime_ns
        && left.size == right.size
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
    cacheable(&meta).then_some(meta).filter(|meta| {
        meta.source_mtime_ns
            .is_none_or(|mtime| mtime == target.source_mtime_ns)
    })
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
                source_mtime_ns: 1,
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

    fn sample_meta(session_id: &str) -> SessionMeta {
        SessionMeta {
            provider_id: "claude".to_string(),
            session_id: session_id.to_string(),
            title: Some("title".to_string()),
            summary: Some("summary".to_string()),
            project_dir: Some("/tmp/project".to_string()),
            created_at: Some(1_000),
            source_mtime_ns: None,
            last_active_at: Some(2_000),
            source_path: Some(format!("/tmp/{session_id}.jsonl")),
            usage: None,
            resume_command: Some(format!("claude --resume {session_id}")),
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
    fn sibling_fingerprint_decoration_preserves_the_source_file_mtime() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("session.json");
        let sibling = temp.path().join(".project_root");
        std::fs::write(&source, "{}").expect("write source");
        std::thread::sleep(std::time::Duration::from_millis(2));
        std::fs::write(&sibling, "/tmp/project").expect("write sibling");

        let mut target = stat_target(&source).expect("source target");
        let source_mtime_ns = target.source_mtime_ns;
        mix_sibling_into_fingerprint_strict(&mut target, &sibling).expect("decorate fingerprint");

        assert_eq!(target.source_mtime_ns, source_mtime_ns);
        assert!(
            target.mtime_ns >= target.source_mtime_ns,
            "the decorated fingerprint may advance without changing source evidence"
        );
    }

    #[test]
    fn source_snapshot_is_only_recorded_when_pre_and_post_parse_stats_match() {
        let before = FileScanTarget {
            path: PathBuf::from("/session.jsonl"),
            mtime_ns: 10,
            source_mtime_ns: 7,
            size: 100,
        };
        assert_eq!(stable_source_mtime_ns(&before, Some(&before)), Some(7));

        let changed = FileScanTarget {
            mtime_ns: 11,
            source_mtime_ns: 8,
            ..before.clone()
        };
        assert_eq!(stable_source_mtime_ns(&before, Some(&changed)), None);
        assert_eq!(stable_source_mtime_ns(&before, None), None);
    }

    #[test]
    fn cache_fallback_rejects_stale_source_mtime_when_sibling_dominates_fingerprint() {
        let key = "/session.json".to_string();
        let current = FileScanTarget {
            path: PathBuf::from(&key),
            source_mtime_ns: 200,
            // A newer sibling can keep this composite value unchanged while
            // the source itself changes.
            mtime_ns: 500,
            size: 42,
        };
        let mut stale_meta = sample_meta("stale");
        stale_meta.source_mtime_ns = Some(100);
        let mut cached = HashMap::new();
        cached.insert(
            key.clone(),
            CachedScanRow {
                mtime_ns: current.mtime_ns,
                size: current.size,
                meta_json: serde_json::to_string(&stale_meta).expect("cache json"),
                cache_version: SCAN_CACHE_VERSION,
            },
        );

        assert!(
            cached_meta_for_target(&cached, &key, &current, &|_| true).is_none(),
            "a composite fingerprint match must not resurrect older source freshness evidence"
        );
    }

    #[test]
    fn legacy_persisted_usage_is_ignored_because_usage_is_runtime_only() {
        let meta: SessionMeta = serde_json::from_str(
            r#"{
                "providerId":"codex",
                "sessionId":"abc",
                "usage":{"totalTokens":42,"totalCostUsd":0.125}
            }"#,
        )
        .expect("parse legacy usage summary");

        assert_eq!(meta.usage, None);
    }
}
