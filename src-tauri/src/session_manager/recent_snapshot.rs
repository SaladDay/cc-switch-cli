//! Bounded, schema-free first-paint snapshots for the Sessions TUI.
//!
//! The scan sidecar's SQLite rows are keyed for incremental revalidation, not
//! for recency. Ordering that table by metadata without a matching index would
//! make startup perform an O(N) temporary sort, while ordering by file mtime is
//! not equivalent to the session's real `last_active_at`/`created_at` order.
//! Instead, an authoritative background scan atomically writes one small JSON
//! snapshot per scope. Opening a Sessions page therefore reads and deserializes
//! at most one page plus its look-ahead row, without changing any database
//! schema.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::de::{Error as _, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use super::{sort_by_recent, SessionMeta, CACHED_PROVIDERS, SCAN_CACHE_FIRST_PAINT_LIMIT};
use crate::config::{
    atomic_write, get_app_config_dir, resolve_config_dir_without_following_user_symlinks,
};
use crate::error::AppError;

const SNAPSHOT_FORMAT_VERSION: u32 = 1;
const SNAPSHOT_FILE_PREFIX: &str = "session-scan-recent-v1";
/// One snapshot contains at most one page plus look-ahead. The row sanitizer
/// caps compact rows at 64 KiB, leaving ample envelope headroom here.
const MAX_SNAPSHOT_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecentSnapshot {
    format_version: u32,
    scan_cache_version: i64,
    provider_id: String,
    #[serde(deserialize_with = "deserialize_bounded_rows")]
    rows: Vec<SessionMeta>,
}

fn deserialize_bounded_rows<'de, D>(deserializer: D) -> Result<Vec<SessionMeta>, D::Error>
where
    D: Deserializer<'de>,
{
    struct BoundedRowsVisitor;

    impl<'de> Visitor<'de> for BoundedRowsVisitor {
        type Value = Vec<SessionMeta>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                formatter,
                "at most {SCAN_CACHE_FIRST_PAINT_LIMIT} session snapshot rows"
            )
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut rows = Vec::with_capacity(SCAN_CACHE_FIRST_PAINT_LIMIT);
            for _ in 0..SCAN_CACHE_FIRST_PAINT_LIMIT {
                match sequence.next_element()? {
                    Some(row) => rows.push(row),
                    None => return Ok(rows),
                }
            }

            // Probe for an extra element without constructing another
            // `SessionMeta`. A hand-edited cache can therefore never retain or
            // transiently allocate a 102nd row object.
            if sequence.next_element::<IgnoredAny>()?.is_some() {
                return Err(A::Error::custom("session snapshot row limit exceeded"));
            }
            Ok(rows)
        }
    }

    deserializer.deserialize_seq(BoundedRowsVisitor)
}

#[derive(Debug, Clone)]
struct SnapshotTombstone {
    generation: u64,
    provider_id: String,
    session_id: String,
    source_path: String,
}

impl SnapshotTombstone {
    fn matches(&self, row: &SessionMeta) -> bool {
        self.provider_id == row.provider_id
            && self.session_id == row.session_id
            && self.source_path == row.source_path.as_deref().unwrap_or_default()
    }
}

#[derive(Default)]
struct SnapshotCoordinator {
    generation: u64,
    next_scan_id: u64,
    active_scans: HashMap<u64, u64>,
    tombstones: Vec<SnapshotTombstone>,
}

fn snapshot_coordinator() -> &'static Mutex<SnapshotCoordinator> {
    static COORDINATOR: OnceLock<Mutex<SnapshotCoordinator>> = OnceLock::new();
    COORDINATOR.get_or_init(|| Mutex::new(SnapshotCoordinator::default()))
}

/// Generation guard for one authoritative scan. Deletes registered after this
/// guard starts are filtered from its snapshot even if the scan read the file
/// before deletion and finishes later.
pub(crate) struct ScanGeneration {
    id: u64,
    started_at: u64,
}

pub(crate) fn begin_scan() -> ScanGeneration {
    let mut coordinator = snapshot_coordinator()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    coordinator.next_scan_id = coordinator.next_scan_id.saturating_add(1);
    let id = coordinator.next_scan_id;
    let started_at = coordinator.generation;
    coordinator.active_scans.insert(id, started_at);
    ScanGeneration { id, started_at }
}

impl Drop for ScanGeneration {
    fn drop(&mut self) {
        let mut coordinator = snapshot_coordinator()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        coordinator.active_scans.remove(&self.id);
        prune_tombstones(&mut coordinator);
    }
}

fn prune_tombstones(coordinator: &mut SnapshotCoordinator) {
    if let Some(oldest_scan) = coordinator.active_scans.values().copied().min() {
        coordinator
            .tombstones
            .retain(|tombstone| tombstone.generation > oldest_scan);
    } else {
        coordinator.tombstones.clear();
    }
}

fn supported_scope(provider_id: &str) -> bool {
    provider_id == "all" || CACHED_PROVIDERS.contains(&provider_id)
}

fn row_belongs_to_scope(row: &SessionMeta, provider_id: &str) -> bool {
    if provider_id == "all" {
        CACHED_PROVIDERS.contains(&row.provider_id.as_str())
    } else {
        row.provider_id == provider_id
    }
}

fn clone_utf8_prefix(value: &str, max_bytes: usize) -> String {
    let mut end = value.len().min(max_bytes);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

/// Build a bounded owned candidate from a borrowed authoritative row. Calling
/// `SessionMeta::clone` first would briefly duplicate arbitrarily large source
/// fields before the canonical sanitizer had a chance to trim or reject them.
fn clone_bounded_manifest_candidate(row: &SessionMeta) -> Option<SessionMeta> {
    let identity_limit = super::paged_manifest::MAX_IDENTITY_FIELD_BYTES;
    let display_limit = super::paged_manifest::MAX_DISPLAY_FIELD_BYTES;
    if row.provider_id.len() > identity_limit || row.session_id.len() > identity_limit {
        return None;
    }
    let source_path = match row.source_path.as_deref() {
        Some(path) if path.len() > identity_limit => return None,
        Some(path) => Some(path.to_owned()),
        None => None,
    };
    let bounded_display =
        |value: Option<&str>| value.map(|value| clone_utf8_prefix(value, display_limit));
    let bounded_action = |value: Option<&str>| {
        value
            .filter(|value| value.len() <= display_limit)
            .map(str::to_owned)
    };

    Some(SessionMeta {
        provider_id: row.provider_id.to_owned(),
        session_id: row.session_id.to_owned(),
        title: bounded_display(row.title.as_deref()),
        summary: bounded_display(row.summary.as_deref()),
        project_dir: bounded_action(row.project_dir.as_deref()),
        created_at: row.created_at,
        last_active_at: row.last_active_at,
        source_path,
        resume_command: bounded_action(row.resume_command.as_deref()),
    })
}

fn snapshot_path(config_dir: &Path, provider_id: &str) -> Option<PathBuf> {
    supported_scope(provider_id)
        .then(|| config_dir.join(format!("{SNAPSHOT_FILE_PREFIX}-{provider_id}.json")))
}

fn invalid_snapshot(path: &Path, reason: &str) -> AppError {
    AppError::Config(format!(
        "invalid recent-session snapshot {}: {reason}",
        path.display()
    ))
}

/// Load one bounded first-paint snapshot. Any corruption, missing file, or
/// incompatible cache version is a cache miss; the authoritative scan still
/// runs and repairs the snapshot in the background.
pub(crate) fn load(provider_id: &str) -> Vec<SessionMeta> {
    let Ok(config_dir) = resolve_config_dir_without_following_user_symlinks(&get_app_config_dir())
    else {
        return Vec::new();
    };
    load_at(&config_dir, provider_id).unwrap_or_default()
}

fn load_at(config_dir: &Path, provider_id: &str) -> Result<Vec<SessionMeta>, AppError> {
    let Some(path) = snapshot_path(config_dir, provider_id) else {
        return Ok(Vec::new());
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(AppError::io(&path, error)),
    };
    let file_len = file
        .metadata()
        .map_err(|error| AppError::io(&path, error))?
        .len();
    if file_len > MAX_SNAPSHOT_BYTES {
        return Err(invalid_snapshot(&path, "byte limit exceeded"));
    }
    let mut source = Vec::with_capacity(usize::try_from(file_len).unwrap_or(0));
    file.take(MAX_SNAPSHOT_BYTES + 1)
        .read_to_end(&mut source)
        .map_err(|error| AppError::io(&path, error))?;
    if source.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err(invalid_snapshot(&path, "byte limit exceeded while reading"));
    }
    let mut snapshot: RecentSnapshot =
        serde_json::from_slice(&source).map_err(|error| AppError::json(&path, error))?;
    if snapshot.format_version != SNAPSHOT_FORMAT_VERSION
        || snapshot.scan_cache_version != super::cache::SCAN_CACHE_VERSION
        || snapshot.provider_id != provider_id
    {
        return Err(invalid_snapshot(&path, "header mismatch"));
    }

    // The writer already stores this exact order and bound. Re-apply both as a
    // cheap defence against a manually edited/corrupt cache file; this is at
    // most 101 rows and never scales with the user's full history.
    let mut bounded_rows = Vec::with_capacity(snapshot.rows.len());
    for row in snapshot.rows.drain(..) {
        if !row_belongs_to_scope(&row, provider_id) {
            return Err(invalid_snapshot(&path, "row scope mismatch"));
        }
        let sanitized = super::paged_manifest::sanitize_manifest_row(row)
            .map_err(|error| AppError::JsonSerialize { source: error })?
            .ok_or_else(|| invalid_snapshot(&path, "row identity exceeds its limit"))?;
        bounded_rows.push(sanitized);
    }
    snapshot.rows = bounded_rows;
    sort_by_recent(&mut snapshot.rows);
    Ok(snapshot.rows)
}

/// Persist the true recency top-K from an authoritative, already-sorted scan.
/// Failures are intentionally non-fatal because this file is only a local
/// acceleration cache.
pub(crate) fn persist_authoritative(
    scan: &ScanGeneration,
    provider_id: &str,
    rows: &[SessionMeta],
) {
    let Ok(config_dir) = resolve_config_dir_without_following_user_symlinks(&get_app_config_dir())
    else {
        return;
    };
    if let Err(error) = persist_authoritative_for_scan_at(&config_dir, scan, provider_id, rows) {
        log::debug!("[SESSION-SCAN] 写入最近会话快照失败 ({provider_id}): {error}");
    }
}

fn persist_authoritative_for_scan_at(
    config_dir: &Path,
    scan: &ScanGeneration,
    provider_id: &str,
    rows: &[SessionMeta],
) -> Result<(), AppError> {
    let coordinator = snapshot_coordinator()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    persist_authoritative_at_filtered(
        config_dir,
        provider_id,
        rows,
        &coordinator.tombstones,
        scan.started_at,
    )
}

#[cfg(test)]
fn persist_authoritative_at(
    config_dir: &Path,
    provider_id: &str,
    rows: &[SessionMeta],
) -> Result<(), AppError> {
    persist_authoritative_at_filtered(config_dir, provider_id, rows, &[], u64::MAX)
}

fn persist_authoritative_at_filtered(
    config_dir: &Path,
    provider_id: &str,
    rows: &[SessionMeta],
    tombstones: &[SnapshotTombstone],
    scan_generation: u64,
) -> Result<(), AppError> {
    if !supported_scope(provider_id) {
        return Ok(());
    }
    let visible = |row: &&SessionMeta| {
        !tombstones
            .iter()
            .any(|tombstone| tombstone.generation > scan_generation && tombstone.matches(row))
    };

    if provider_id == "all" {
        write_scope(config_dir, "all", rows.iter().filter(visible))?;
        for scope in CACHED_PROVIDERS {
            write_scope(
                config_dir,
                scope,
                rows.iter()
                    .filter(visible)
                    .filter(|row| row.provider_id == scope),
            )?;
        }
        return Ok(());
    }

    write_scope(config_dir, provider_id, rows.iter().filter(visible))?;

    // If every per-provider snapshot exists, their union is sufficient to
    // derive the global top-K exactly: no provider can contribute its 102nd row
    // before its own first 101 rows. Missing scopes leave the last known all
    // snapshot untouched rather than publishing an incomplete global view.
    let mut all_candidates =
        Vec::with_capacity(CACHED_PROVIDERS.len() * SCAN_CACHE_FIRST_PAINT_LIMIT);
    for scope in CACHED_PROVIDERS {
        let scope_rows = match load_at(config_dir, scope) {
            Ok(rows) => rows,
            Err(error) => {
                // A disposable sibling cache must not turn an authoritative
                // provider scan into a failure or replace the last complete
                // global snapshot with a partial union.
                log::debug!("[SESSION-SCAN] 跳过损坏的最近会话快照 ({scope}): {error}");
                return Ok(());
            }
        };
        if scope_rows.is_empty() && !snapshot_path(config_dir, scope).is_some_and(|p| p.exists()) {
            return Ok(());
        }
        all_candidates.extend(scope_rows);
    }
    all_candidates.retain(|row| {
        !tombstones
            .iter()
            .any(|tombstone| tombstone.generation > scan_generation && tombstone.matches(row))
    });
    sort_by_recent(&mut all_candidates);
    all_candidates.truncate(SCAN_CACHE_FIRST_PAINT_LIMIT);
    write_scope(config_dir, "all", all_candidates.iter())
}

fn write_scope<'a>(
    config_dir: &Path,
    provider_id: &str,
    rows: impl Iterator<Item = &'a SessionMeta>,
) -> Result<(), AppError> {
    let Some(path) = snapshot_path(config_dir, provider_id) else {
        return Ok(());
    };
    let mut bounded_rows = Vec::with_capacity(SCAN_CACHE_FIRST_PAINT_LIMIT);
    for row in rows {
        if bounded_rows.len() == SCAN_CACHE_FIRST_PAINT_LIMIT {
            break;
        }
        if !row_belongs_to_scope(row, provider_id) {
            continue;
        }
        let Some(candidate) = clone_bounded_manifest_candidate(row) else {
            continue;
        };
        if let Some(row) = super::paged_manifest::sanitize_manifest_row(candidate)
            .map_err(|error| AppError::JsonSerialize { source: error })?
        {
            bounded_rows.push(row);
        }
    }
    let snapshot = RecentSnapshot {
        format_version: SNAPSHOT_FORMAT_VERSION,
        scan_cache_version: super::cache::SCAN_CACHE_VERSION,
        provider_id: provider_id.to_string(),
        rows: bounded_rows,
    };
    let serialized =
        serde_json::to_vec(&snapshot).map_err(|error| AppError::JsonSerialize { source: error })?;
    if serialized.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err(AppError::Config(
            "bounded session snapshot exceeded its byte limit".to_string(),
        ));
    }
    atomic_write(&path, &serialized)
}

/// Remove a deleted session from the bounded provider and global snapshots so
/// stale first paint cannot briefly resurrect it before revalidation finishes.
pub(crate) fn purge(provider_id: &str, session_id: &str, source_path: &str) {
    let Ok(config_dir) = resolve_config_dir_without_following_user_symlinks(&get_app_config_dir())
    else {
        return;
    };
    register_delete_and_purge_at(&config_dir, provider_id, session_id, source_path);
}

fn register_delete_and_purge_at(
    config_dir: &Path,
    provider_id: &str,
    session_id: &str,
    source_path: &str,
) {
    let mut coordinator = snapshot_coordinator()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    coordinator.generation = coordinator.generation.saturating_add(1);
    let generation = coordinator.generation;
    coordinator.tombstones.push(SnapshotTombstone {
        generation,
        provider_id: provider_id.to_string(),
        session_id: session_id.to_string(),
        source_path: source_path.to_string(),
    });
    for scope in [provider_id, "all"] {
        let Ok(mut rows) = load_at(config_dir, scope) else {
            continue;
        };
        let before = rows.len();
        rows.retain(|row| {
            row.provider_id != provider_id
                || row.session_id != session_id
                || row.source_path.as_deref().unwrap_or_default() != source_path
        });
        if rows.len() != before {
            let _ = write_scope(config_dir, scope, rows.iter());
        }
    }
    prune_tombstones(&mut coordinator);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn meta(provider: &str, id: &str, recency: i64) -> SessionMeta {
        SessionMeta {
            provider_id: provider.to_string(),
            session_id: id.to_string(),
            last_active_at: Some(recency),
            source_path: Some(format!("/{provider}/{id}.jsonl")),
            ..SessionMeta::default()
        }
    }

    #[test]
    fn snapshot_is_bounded_and_uses_session_recency() {
        let temp = tempdir().expect("tempdir");
        let mut rows: Vec<_> = (0..150)
            .map(|index| meta("claude", &format!("s-{index}"), index))
            .collect();
        sort_by_recent(&mut rows);

        persist_authoritative_at(temp.path(), "claude", &rows).expect("persist snapshot");
        let loaded = load_at(temp.path(), "claude").expect("load snapshot");

        assert_eq!(loaded.len(), SCAN_CACHE_FIRST_PAINT_LIMIT);
        assert_eq!(loaded[0].session_id, "s-149");
        assert_eq!(loaded[100].session_id, "s-49");
    }

    #[test]
    fn oversized_snapshot_is_rejected_before_deserialization() {
        let temp = tempdir().expect("tempdir");
        let path = snapshot_path(temp.path(), "claude").expect("snapshot path");
        let file = File::create(&path).expect("create oversized snapshot");
        file.set_len(MAX_SNAPSHOT_BYTES + 1)
            .expect("extend sparse snapshot");

        assert!(load_at(temp.path(), "claude").is_err());
    }

    #[test]
    fn snapshot_rejects_more_than_one_page_before_retaining_extra_rows() {
        let temp = tempdir().expect("tempdir");
        let path = snapshot_path(temp.path(), "claude").expect("snapshot path");
        let rows = vec![SessionMeta::default(); SCAN_CACHE_FIRST_PAINT_LIMIT + 1];
        let snapshot = RecentSnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            scan_cache_version: crate::session_manager::cache::SCAN_CACHE_VERSION,
            provider_id: "claude".to_string(),
            rows,
        };
        atomic_write(
            &path,
            &serde_json::to_vec(&snapshot).expect("serialize oversized row count"),
        )
        .expect("write snapshot");

        assert!(load_at(temp.path(), "claude").is_err());
    }

    #[test]
    fn snapshot_writer_sanitizes_display_and_action_fields() {
        let temp = tempdir().expect("tempdir");
        let mut row = meta("claude", "bounded", 1);
        row.title = Some("界".repeat(20_000));
        row.resume_command = Some("x".repeat(20_000));

        write_scope(temp.path(), "claude", std::iter::once(&row)).expect("write snapshot");
        let loaded = load_at(temp.path(), "claude").expect("load snapshot");

        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].title.as_ref().is_some_and(|title| {
            title.len() <= crate::session_manager::paged_manifest::MAX_DISPLAY_FIELD_BYTES
        }));
        assert!(loaded[0].resume_command.is_none());
    }

    #[test]
    fn hand_edited_snapshot_is_sanitized_before_reaching_the_ui() {
        let temp = tempdir().expect("tempdir");
        let path = snapshot_path(temp.path(), "claude").expect("snapshot path");
        let mut row = meta("claude", "bounded", 1);
        row.title = Some("界".repeat(20_000));
        row.project_dir = Some("p".repeat(20_000));
        row.resume_command = Some("x".repeat(20_000));
        let snapshot = RecentSnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            scan_cache_version: crate::session_manager::cache::SCAN_CACHE_VERSION,
            provider_id: "claude".to_string(),
            rows: vec![row],
        };
        atomic_write(
            &path,
            &serde_json::to_vec(&snapshot).expect("serialize hand-edited snapshot"),
        )
        .expect("write hand-edited snapshot");

        let loaded = load_at(temp.path(), "claude").expect("load sanitized snapshot");

        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].title.as_ref().is_some_and(|title| {
            title.len() <= crate::session_manager::paged_manifest::MAX_DISPLAY_FIELD_BYTES
        }));
        assert!(loaded[0].project_dir.is_none());
        assert!(loaded[0].resume_command.is_none());
    }

    #[test]
    fn hand_edited_snapshot_cannot_cross_provider_scopes() {
        let temp = tempdir().expect("tempdir");
        let path = snapshot_path(temp.path(), "claude").expect("snapshot path");
        let snapshot = RecentSnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            scan_cache_version: crate::session_manager::cache::SCAN_CACHE_VERSION,
            provider_id: "claude".to_string(),
            rows: vec![meta("codex", "wrong-scope", 1)],
        };
        atomic_write(
            &path,
            &serde_json::to_vec(&snapshot).expect("serialize wrong-scope snapshot"),
        )
        .expect("write wrong-scope snapshot");

        assert!(load_at(temp.path(), "claude").is_err());
    }

    #[test]
    fn writer_emits_at_most_one_sanitized_page_within_the_byte_limit() {
        let temp = tempdir().expect("tempdir");
        let rows: Vec<_> = (0..150)
            .map(|index| meta("claude", &format!("s-{index}"), index))
            .collect();

        write_scope(temp.path(), "claude", rows.iter()).expect("write snapshot");

        let path = snapshot_path(temp.path(), "claude").expect("snapshot path");
        let serialized = std::fs::read(&path).expect("read writer output");
        assert!(serialized.len() as u64 <= MAX_SNAPSHOT_BYTES);
        let snapshot: RecentSnapshot =
            serde_json::from_slice(&serialized).expect("parse writer output");
        assert_eq!(snapshot.rows.len(), SCAN_CACHE_FIRST_PAINT_LIMIT);
        for row in snapshot.rows {
            let sanitized =
                crate::session_manager::paged_manifest::sanitize_manifest_row(row.clone())
                    .expect("sanitize writer row");
            assert_eq!(sanitized.as_ref(), Some(&row));
        }
    }

    #[test]
    fn corrupt_sibling_cache_does_not_fail_authoritative_provider_persist() {
        let temp = tempdir().expect("tempdir");
        let old_global = meta("codex", "old-global", 1);
        write_scope(temp.path(), "all", std::iter::once(&old_global))
            .expect("write old global snapshot");
        let corrupt_path = snapshot_path(temp.path(), "codex").expect("snapshot path");
        atomic_write(&corrupt_path, b"{").expect("write corrupt sibling snapshot");

        let new_provider = meta("claude", "new-provider", 2);
        persist_authoritative_at(temp.path(), "claude", &[new_provider])
            .expect("persist authoritative provider rows");

        let provider = load_at(temp.path(), "claude").expect("load provider snapshot");
        assert_eq!(provider.len(), 1);
        assert_eq!(provider[0].session_id, "new-provider");
        let global = load_at(temp.path(), "all").expect("load old global snapshot");
        assert_eq!(global.len(), 1);
        assert_eq!(global[0].session_id, "old-global");
    }

    #[test]
    fn all_snapshot_uses_session_recency_instead_of_source_path_or_input_order() {
        let temp = tempdir().expect("tempdir");
        let mut old = meta("claude", "old", 10);
        old.source_path = Some("/a-first-by-path.jsonl".to_string());
        let mut new = meta("codex", "new", 20);
        new.source_path = Some("/z-last-by-path.jsonl".to_string());
        let mut rows = vec![old, new];
        sort_by_recent(&mut rows);

        persist_authoritative_at(temp.path(), "all", &rows).expect("persist all snapshot");
        let loaded = load_at(temp.path(), "all").expect("load all snapshot");

        assert_eq!(loaded[0].session_id, "new");
        assert_eq!(loaded[1].session_id, "old");
    }

    #[test]
    fn purge_updates_provider_and_all_snapshots() {
        let temp = tempdir().expect("tempdir");
        let mut rows = vec![meta("claude", "gone", 20), meta("codex", "kept", 10)];
        sort_by_recent(&mut rows);
        persist_authoritative_at(temp.path(), "all", &rows).expect("persist all");

        let mut provider = load_at(temp.path(), "claude").expect("load provider");
        provider.retain(|row| row.session_id != "gone");
        write_scope(temp.path(), "claude", provider.iter()).expect("purge provider");
        let mut all = load_at(temp.path(), "all").expect("load all");
        all.retain(|row| row.session_id != "gone");
        write_scope(temp.path(), "all", all.iter()).expect("purge all");

        assert!(load_at(temp.path(), "claude").expect("reload").is_empty());
        assert_eq!(load_at(temp.path(), "all").expect("reload").len(), 1);
    }

    #[test]
    fn delete_after_scan_start_cannot_be_reintroduced_by_late_snapshot_persist() {
        let temp = tempdir().expect("tempdir");
        let scan = begin_scan();
        let gone = meta("claude", "gone", 20);
        let kept = meta("claude", "kept", 10);

        // The scan has already captured `gone`; deletion lands and purges any
        // old snapshot before that stale scan reaches its persist step.
        register_delete_and_purge_at(
            temp.path(),
            "claude",
            "gone",
            gone.source_path.as_deref().expect("gone source"),
        );
        persist_authoritative_for_scan_at(temp.path(), &scan, "claude", &[gone, kept])
            .expect("late persist");

        let loaded = load_at(temp.path(), "claude").expect("load snapshot");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].session_id, "kept");
    }
}
