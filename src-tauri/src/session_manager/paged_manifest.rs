//! Schema-free, page-oriented storage for very large Sessions lists.
//!
//! The regular scan cache is optimized for per-file revalidation. It cannot
//! provide recency pages without visiting and sorting all of its rows. This
//! module stores the result of an authoritative scan as immutable generations:
//!
//! - every disk page contains at most [`PAGE_SIZE`] rows;
//! - `current.json` is a small pointer atomically replaced only after every page
//!   in a generation is durable;
//! - builders spill sorted runs and merge at a bounded fan-in, so neither RAM
//!   nor open file descriptors grows with the number of sessions;
//! - a reader leases one immutable generation, so a refresh cannot move page
//!   boundaries underneath an in-progress paging gesture;
//! - a newer build cancels the previous build for the same scope, and deletes
//!   leave tombstones that an older build cannot publish over.
//!
//! This is a local, disposable JSON/JSONL cache. It deliberately does not add a
//! table, index, migration, or any other schema to either CC-Switch's database
//! or a provider-owned database.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;

use super::{SessionMeta, CACHED_PROVIDERS};
use crate::config::{
    atomic_write, get_app_config_dir, resolve_config_dir_without_following_user_symlinks,
    write_json_file,
};

/// Rows presented on one logical Sessions page.
pub(crate) const PAGE_SIZE: usize = 100;

const FORMAT_VERSION: u32 = 1;
const ROOT_DIR: &str = "session-pages-v1";
const QUERY_ROOT_DIR: &str = "session-query-pages-v1";
const CLI_ROOT_DIR: &str = "session-cli-pages-v1";
const POINTER_FILE: &str = "current.json";
const HEADER_FILE: &str = "manifest.json";
const COORDINATOR_FILE: &str = ".coord.json";
const READ_FLOOR_FILE: &str = ".read-floor.json";
const SCOPE_LOCK_FILE: &str = ".scope.lock";
const BUILD_OWNER_FILE: &str = ".owner.lock";
const GENERATION_LEASE_FILE: &str = ".lease.lock";
const NAMESPACE_LOCK_FILE: &str = ".namespace.lock";
const QUERY_ROOT_LOCK_FILE: &str = ".query-root.lock";
const CLI_ROOT_LOCK_FILE: &str = ".cli-root.lock";
const DEFAULT_SPILL_ROWS: usize = PAGE_SIZE * 8;
const DEFAULT_MERGE_FAN_IN: usize = 32;
const MAX_POINTER_BYTES: u64 = 64 * 1024;
const MAX_HEADER_BYTES: u64 = 64 * 1024;
const MAX_PAGE_BYTES: u64 = 8 * 1024 * 1024;
/// Identity fields are never truncated because they define deduplication and
/// deletion semantics. Rows with a larger identity are omitted from this
/// disposable cache instead.
pub(super) const MAX_IDENTITY_FIELD_BYTES: usize = 16 * 1024;
/// Non-identity strings are bounded as well. Display-only title/summary values
/// may be shortened; actionable command/path values are instead removed when
/// oversized so a truncated value can never be executed or used as a cwd.
pub(super) const MAX_DISPLAY_FIELD_BYTES: usize = 16 * 1024;
/// This leaves more than 1.5 MiB of envelope headroom in a 100-row page.
const MAX_SESSION_META_JSON_BYTES: usize = 64 * 1024;
const MAX_PAGE_ENVELOPE_BYTES: usize = 256 * 1024;
const _: () = assert!(
    PAGE_SIZE * MAX_SESSION_META_JSON_BYTES + MAX_PAGE_ENVELOPE_BYTES <= MAX_PAGE_BYTES as usize
);
const MAX_COORDINATOR_BYTES: u64 = 16 * 1024 * 1024;
const MAX_READ_FLOOR_BYTES: u64 = 64 * 1024;
const MAX_TOMBSTONES: usize = 4_096;

#[derive(Debug, Error)]
pub(crate) enum ManifestError {
    #[error("unsupported session manifest scope: {0}")]
    UnsupportedScope(String),
    #[error("session row provider {provider} does not belong to scope {scope}")]
    RowOutsideScope { scope: String, provider: String },
    #[error("session manifest build was superseded or cancelled")]
    Cancelled,
    #[error("invalid session manifest build options: {0}")]
    InvalidOptions(String),
    #[error("session manifest I/O failed at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("session manifest JSON failed at {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("corrupt session manifest: {0}")]
    Corrupt(String),
}

impl ManifestError {
    fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.as_ref().display().to_string(),
            source,
        }
    }

    fn json(path: impl AsRef<Path>, source: serde_json::Error) -> Self {
        Self::Json {
            path: path.as_ref().display().to_string(),
            source,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct BuildOptions {
    spill_rows: usize,
    merge_fan_in: usize,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            spill_rows: DEFAULT_SPILL_ROWS,
            merge_fan_in: DEFAULT_MERGE_FAN_IN,
        }
    }
}

impl BuildOptions {
    fn validate(self) -> Result<Self, ManifestError> {
        if self.spill_rows == 0 {
            return Err(ManifestError::InvalidOptions(
                "spill_rows must be greater than zero".to_string(),
            ));
        }
        if self.merge_fan_in < 2 {
            return Err(ManifestError::InvalidOptions(
                "merge_fan_in must be at least two".to_string(),
            ));
        }
        Ok(self)
    }
}

/// One materialized page from an immutable generation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ManifestPage {
    pub generation: String,
    pub page_index: usize,
    pub total_rows: usize,
    pub rows: Vec<SessionMeta>,
    pub has_next: bool,
}

/// Result returned by an atomic publication. `first_page` is validated from
/// the owner-leased staging bytes that become this exact immutable generation,
/// so a concurrent refresh cannot make this response internally mixed.
#[derive(Debug, Clone)]
pub(crate) struct PublishedManifest {
    pub generation: String,
    /// Monotonic within one physical scope. UI consumers use this to reject a
    /// delayed purge/refresh result after a newer generation is already known.
    pub build_epoch: u64,
    pub total_rows: usize,
    pub page_count: usize,
    pub first_page: ManifestPage,
    /// Acquired while the publication still owns the scope lock. There is no
    /// publish-to-open window in which another process can collect this
    /// generation before the caller pins it.
    pub reader: ManifestReader,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ManifestHeader {
    format_version: u32,
    scan_cache_version: i64,
    #[serde(default)]
    epoch_domain: String,
    #[serde(default)]
    build_epoch: u64,
    scope: String,
    generation: String,
    page_size: usize,
    total_rows: usize,
    page_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiskPage {
    format_version: u32,
    scan_cache_version: i64,
    scope: String,
    generation: String,
    page_index: usize,
    rows: Vec<SessionMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ScopeKey {
    root: PathBuf,
    scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tombstone {
    generation: u64,
    provider_id: String,
    session_id: String,
    source_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiskCoordinator {
    format_version: u32,
    #[serde(default)]
    epoch_domain: String,
    build_epoch: u64,
    delete_generation: u64,
    #[serde(default)]
    min_readable_epoch: u64,
    tombstones: Vec<Tombstone>,
}

impl Default for DiskCoordinator {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            epoch_domain: next_epoch_domain(),
            build_epoch: 0,
            delete_generation: 0,
            min_readable_epoch: 0,
            tombstones: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadFloor {
    format_version: u32,
    min_readable_epoch: u64,
}

#[derive(Default)]
struct TombstoneSet {
    by_provider: HashMap<String, HashMap<String, HashSet<String>>>,
}

impl TombstoneSet {
    fn from_items(items: &[Tombstone]) -> Self {
        let mut set = Self::default();
        for item in items {
            set.by_provider
                .entry(item.provider_id.clone())
                .or_default()
                .entry(item.session_id.clone())
                .or_default()
                .insert(item.source_path.clone());
        }
        set
    }

    fn contains(&self, row: &SessionMeta) -> bool {
        self.by_provider
            .get(row.provider_id.as_str())
            .and_then(|sessions| sessions.get(row.session_id.as_str()))
            .is_some_and(|paths| paths.contains(row.source_path.as_deref().unwrap_or_default()))
    }
}

#[derive(Default)]
struct ScopeState {
    active_cancel: Option<Weak<AtomicBool>>,
}

#[derive(Default)]
struct Coordinator {
    scopes: HashMap<ScopeKey, ScopeState>,
}

fn coordinator() -> &'static Mutex<Coordinator> {
    static COORDINATOR: OnceLock<Mutex<Coordinator>> = OnceLock::new();
    COORDINATOR.get_or_init(|| Mutex::new(Coordinator::default()))
}

#[derive(Debug)]
struct FileLock {
    file: File,
}

impl FileLock {
    fn shared(path: &Path) -> Result<Self, ManifestError> {
        let file = open_lock_file(path)?;
        file.lock_shared()
            .map_err(|error| ManifestError::io(path, error))?;
        Ok(Self { file })
    }

    fn exclusive(path: &Path) -> Result<Self, ManifestError> {
        let file = open_lock_file(path)?;
        file.lock()
            .map_err(|error| ManifestError::io(path, error))?;
        Ok(Self { file })
    }

    fn try_exclusive(path: &Path) -> Result<Option<Self>, ManifestError> {
        let file = open_lock_file(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { file })),
            Err(error) => {
                let error: std::io::Error = error.into();
                if error.kind() == std::io::ErrorKind::WouldBlock {
                    Ok(None)
                } else {
                    Err(ManifestError::io(path, error))
                }
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[derive(Debug)]
struct NamespaceCleanup {
    namespace_parent: PathBuf,
    namespace_root: PathBuf,
    root_lock_file: &'static str,
    log_target: &'static str,
    #[cfg(test)]
    deletion_barriers: Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>,
}

impl Drop for NamespaceCleanup {
    fn drop(&mut self) {
        let retired = (|| -> Result<Option<PathBuf>, ManifestError> {
            let _root_lock = FileLock::exclusive(&self.namespace_parent.join(self.root_lock_file))?;
            if !self.namespace_root.is_dir() {
                return Ok(None);
            }
            let Some(namespace_lock) =
                FileLock::try_exclusive(&self.namespace_root.join(NAMESPACE_LOCK_FILE))?
            else {
                // This should only happen if another process opened this exact
                // namespace. Leave it intact; the next CLI startup will retry.
                return Ok(None);
            };
            let mut batch = None;
            let mut sequence = 0usize;
            quarantine_into_batch(
                &self.namespace_root,
                &self.namespace_parent,
                &mut batch,
                &mut sequence,
            );
            drop(namespace_lock);
            if self.namespace_root.exists() {
                return Err(ManifestError::Corrupt(
                    "namespace quarantine rename did not complete".to_string(),
                ));
            }
            sync_directory(&self.namespace_parent)?;
            Ok(batch)
        })();
        match retired {
            Ok(Some(path)) => {
                #[cfg(test)]
                remove_namespace_artifact_in_background(path, self.deletion_barriers.clone());
                #[cfg(not(test))]
                remove_namespace_artifact_in_background(path);
            }
            Ok(None) => {}
            Err(error) => log::debug!(
                "[{}] failed to retire completed namespace {}: {error}",
                self.log_target,
                self.namespace_root.display()
            ),
        }
    }
}

/// Local store for page manifests. Cloning it is cheap and does not open files.
#[derive(Debug, Clone)]
pub(crate) struct PagedManifestStore {
    root: PathBuf,
    /// Query and one-shot CLI stores retain a namespace lease through every
    /// cloned reader. Authoritative stores do not need a root-level lease.
    _root_lease: Option<Arc<FileLock>>,
    /// Field order is deliberate: the shared namespace lock above must be
    /// released before the last namespaced clone runs its cleanup guard.
    _namespace_cleanup: Option<Arc<NamespaceCleanup>>,
}

impl PagedManifestStore {
    pub(crate) fn open() -> Result<Self, ManifestError> {
        let config_dir = resolve_config_dir_without_following_user_symlinks(&get_app_config_dir())
            .map_err(|error| ManifestError::Corrupt(error.to_string()))?;
        Self::open_at(&config_dir)
    }

    pub(crate) fn open_at(config_dir: &Path) -> Result<Self, ManifestError> {
        Self::open_named_at(config_dir, ROOT_DIR)
    }

    fn open_named_at(config_dir: &Path, root_dir: &str) -> Result<Self, ManifestError> {
        let root = config_dir.join(root_dir);
        Self::open_root(root, None, None)
    }

    fn open_root(
        root: PathBuf,
        root_lease: Option<Arc<FileLock>>,
        namespace_cleanup: Option<Arc<NamespaceCleanup>>,
    ) -> Result<Self, ManifestError> {
        create_private_dir(&root)?;
        Ok(Self {
            root,
            _root_lease: root_lease,
            _namespace_cleanup: namespace_cleanup,
        })
    }

    /// Start a new build. Starting or cancelling a build is scoped: a refresh
    /// for `claude` does not interrupt a refresh for `codex`.
    pub(crate) fn begin_build(&self, scope: &str) -> Result<PagedManifestBuilder, ManifestError> {
        self.begin_build_with_options(scope, BuildOptions::default())
    }

    fn begin_build_with_options(
        &self,
        scope: &str,
        options: BuildOptions,
    ) -> Result<PagedManifestBuilder, ManifestError> {
        validate_scope(scope)?;
        let options = options.validate()?;
        let key = self.scope_key(scope);
        let cancel = Arc::new(AtomicBool::new(false));
        let scope_dir = self.scope_dir(scope);
        create_private_dir(&scope_dir)?;
        let _scope_lock = FileLock::exclusive(&self.scope_lock_path(scope))?;
        self.cleanup_scope_artifacts_locked(scope);

        let mut disk = self.read_or_recover_coordinator_locked(scope)?;
        disk.build_epoch = next_epoch(disk.build_epoch)?;
        self.write_disk_coordinator_locked(scope, &disk)?;
        let epoch_domain = disk.epoch_domain.clone();
        let epoch = disk.build_epoch;
        let delete_generation = disk.delete_generation;

        {
            let mut all = coordinator()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = all.scopes.entry(key.clone()).or_default();
            if let Some(previous) = state.active_cancel.take().and_then(|weak| weak.upgrade()) {
                previous.store(true, AtomicOrdering::Release);
            }
            state.active_cancel = Some(Arc::downgrade(&cancel));
        }

        let generation = next_generation();
        let staging_dir = scope_dir.join(format!(".{generation}.building"));
        create_private_dir(&staging_dir)?;
        let owner_lock = FileLock::exclusive(&staging_dir.join(BUILD_OWNER_FILE))?;
        drop(open_lock_file(&staging_dir.join(GENERATION_LEASE_FILE))?);

        Ok(PagedManifestBuilder {
            store: self.clone(),
            key,
            scope: scope.to_string(),
            generation,
            epoch_domain,
            epoch,
            started_delete_generation: delete_generation,
            cancel,
            options,
            staging_dir,
            owner_lock: Some(owner_lock),
            buffer: Vec::with_capacity(options.spill_rows.min(DEFAULT_SPILL_ROWS)),
            run_levels: Vec::new(),
            next_run_id: 0,
            published: false,
            expected_source: None,
            identity_row_enrichers: Vec::new(),
            #[cfg(test)]
            peak_retained_run_paths: 0,
            #[cfg(test)]
            corrupt_page_before_validation: None,
            #[cfg(test)]
            validation_barriers: None,
        })
    }

    /// Cancel only the currently active build for this scope. A builder checks
    /// this flag while spilling, merging, writing pages, and immediately before
    /// updating the pointer.
    pub(crate) fn cancel(&self, scope: &str) {
        if validate_scope(scope).is_err() {
            return;
        }
        if self.scope_dir(scope).is_dir() {
            let result = (|| -> Result<(), ManifestError> {
                let _scope_lock = FileLock::exclusive(&self.scope_lock_path(scope))?;
                let mut disk = self.read_or_recover_coordinator_locked(scope)?;
                disk.build_epoch = next_epoch(disk.build_epoch)?;
                self.write_disk_coordinator_locked(scope, &disk)
            })();
            if let Err(error) = result {
                log::debug!("[SESSION-PAGES] failed to persist cancellation for {scope}: {error}");
            }
        }
        let key = self.scope_key(scope);
        let mut all = coordinator()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = all.scopes.entry(key).or_default();
        if let Some(active) = state.active_cancel.take().and_then(|weak| weak.upgrade()) {
            active.store(true, AtomicOrdering::Release);
        }
    }

    /// Open the current immutable generation and hold a reader lease. Corrupt,
    /// partial, incompatible, or missing cache data is a cache miss (`None`).
    pub(crate) fn open_reader(&self, scope: &str) -> Option<ManifestReader> {
        validate_scope(scope).ok()?;
        let _scope_lock = FileLock::shared(&self.scope_lock_path(scope)).ok()?;
        let header = self.read_current_header(scope).ok().flatten()?;
        self.lease_reader(header)
    }

    /// Open a specific immutable generation returned by an earlier page load.
    /// This is the stable path for asynchronous `LoadPage` requests: a refresh
    /// may advance `current.json`, but cannot move the requested page boundary.
    pub(crate) fn open_generation(&self, scope: &str, generation: &str) -> Option<ManifestReader> {
        validate_scope(scope).ok()?;
        if !valid_generation(generation) {
            return None;
        }
        let _scope_lock = FileLock::shared(&self.scope_lock_path(scope)).ok()?;
        let header_path = self.generation_dir(scope, generation).join(HEADER_FILE);
        let header: ManifestHeader = read_json_limited(&header_path, MAX_HEADER_BYTES).ok()?;
        validate_header(&header, scope).ok()?;
        if header.generation != generation {
            return None;
        }
        if !self.header_is_readable_locked(&header).ok()? {
            return None;
        }
        self.lease_reader(header)
    }

    fn lease_reader(&self, header: ManifestHeader) -> Option<ManifestReader> {
        let lease_path = self
            .generation_dir(&header.scope, &header.generation)
            .join(GENERATION_LEASE_FILE);
        let lease = Arc::new(FileLock::shared(&lease_path).ok()?);
        Some(ManifestReader {
            _lease: lease,
            header,
            store: self.clone(),
        })
    }

    /// Convenience read of one page from the current generation. Callers that
    /// load multiple pages should retain [`ManifestReader`] instead.
    pub(crate) fn load_page(&self, scope: &str, page_index: usize) -> Option<ManifestPage> {
        self.open_reader(scope)?.load_page(page_index)
    }

    /// Register a deletion, cancel an older build, and rebuild the current
    /// immutable generation without the deleted identity. The rebuild streams
    /// one bounded page at a time. If no valid generation exists, the tombstone
    /// remains and filters the next successful authoritative build.
    pub(crate) fn purge_identity(
        &self,
        scope: &str,
        provider_id: &str,
        session_id: &str,
        source_path: &str,
    ) -> Result<Option<PublishedManifest>, ManifestError> {
        validate_scope(scope)?;
        if !identity_values_are_bounded(provider_id, session_id, Some(source_path)) {
            return Ok(None);
        }
        let key = self.scope_key(scope);
        create_private_dir(&self.scope_dir(scope))?;
        {
            let _scope_lock = FileLock::exclusive(&self.scope_lock_path(scope))?;
            let mut disk = self.read_or_recover_coordinator_locked(scope)?;
            disk.delete_generation = next_epoch(disk.delete_generation)?;
            disk.build_epoch = next_epoch(disk.build_epoch)?;
            let generation = disk.delete_generation;
            self.record_tombstone_bounded(
                scope,
                &mut disk,
                Tombstone {
                    generation,
                    provider_id: provider_id.to_string(),
                    session_id: session_id.to_string(),
                    source_path: source_path.to_string(),
                },
            )?;
            self.write_disk_coordinator_locked(scope, &disk)?;

            let mut all = coordinator()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = all.scopes.entry(key).or_default();
            if let Some(active) = state.active_cancel.take().and_then(|weak| weak.upgrade()) {
                active.store(true, AtomicOrdering::Release);
            }
        }

        let mut builder = self.begin_build(scope)?;
        // The winning build epoch is established before the source pointer is
        // opened. Therefore an older refresh cannot publish between source
        // capture and this purge, and any newer builder cancels this one.
        let Some(reader) = self.open_reader(scope) else {
            return Ok(None);
        };
        builder.bind_source(&reader)?;
        for page_index in 0..reader.page_count() {
            builder.ensure_active()?;
            let page = reader.load_page(page_index).ok_or_else(|| {
                ManifestError::Corrupt("page disappeared during purge".to_string())
            })?;
            for row in page.rows {
                if row.provider_id != provider_id
                    || row.session_id != session_id
                    || row.source_path.as_deref().unwrap_or_default() != source_path
                {
                    builder.push(row)?;
                }
            }
        }
        builder.publish().map(Some)
    }

    fn read_current_header(&self, scope: &str) -> Result<Option<ManifestHeader>, ManifestError> {
        let pointer_path = self.scope_dir(scope).join(POINTER_FILE);
        if !pointer_path.exists() {
            return Ok(None);
        }
        let pointer: ManifestHeader = read_json_limited(&pointer_path, MAX_POINTER_BYTES)?;
        validate_header(&pointer, scope)?;
        if !valid_generation(&pointer.generation) {
            return Err(ManifestError::Corrupt(
                "current pointer contains an invalid generation".to_string(),
            ));
        }
        let header_path = self
            .generation_dir(scope, &pointer.generation)
            .join(HEADER_FILE);
        let header: ManifestHeader = read_json_limited(&header_path, MAX_HEADER_BYTES)?;
        validate_header(&header, scope)?;
        if pointer != header {
            return Err(ManifestError::Corrupt(
                "current pointer does not match immutable manifest header".to_string(),
            ));
        }
        if !self.header_is_readable_locked(&header)? {
            return Ok(None);
        }
        Ok(Some(header))
    }

    fn header_is_readable_locked(&self, header: &ManifestHeader) -> Result<bool, ManifestError> {
        let disk = self.read_disk_coordinator_locked(&header.scope)?;
        let min_readable_epoch = self
            .read_min_readable_epoch_locked(&header.scope)?
            .max(disk.min_readable_epoch);
        Ok(valid_epoch_domain(&disk.epoch_domain)
            && header.epoch_domain == disk.epoch_domain
            && header.build_epoch >= min_readable_epoch)
    }

    fn read_page(&self, header: &ManifestHeader, page_index: usize) -> Option<ManifestPage> {
        let _scope_lock = FileLock::shared(&self.scope_lock_path(&header.scope)).ok()?;
        if !self.header_is_readable_locked(header).ok()? {
            return None;
        }
        if header.page_count == 0 && page_index == 0 {
            return Some(ManifestPage {
                generation: header.generation.clone(),
                page_index,
                total_rows: 0,
                rows: Vec::new(),
                has_next: false,
            });
        }
        if page_index >= header.page_count {
            return None;
        }
        let rows = self.read_disk_page(header, page_index).ok()?;
        let has_next = page_index + 1 < header.page_count;
        Some(ManifestPage {
            generation: header.generation.clone(),
            page_index,
            total_rows: header.total_rows,
            rows,
            has_next,
        })
    }

    fn read_disk_page(
        &self,
        header: &ManifestHeader,
        page_index: usize,
    ) -> Result<Vec<SessionMeta>, ManifestError> {
        self.read_disk_page_from_dir(
            header,
            page_index,
            &self.generation_dir(&header.scope, &header.generation),
        )
    }

    fn read_disk_page_from_dir(
        &self,
        header: &ManifestHeader,
        page_index: usize,
        generation_dir: &Path,
    ) -> Result<Vec<SessionMeta>, ManifestError> {
        let path = generation_dir.join(page_file_name(page_index));
        let page: DiskPage = read_json_limited(&path, MAX_PAGE_BYTES)?;
        if page.format_version != FORMAT_VERSION
            || page.scan_cache_version != super::cache::SCAN_CACHE_VERSION
            || page.scope != header.scope
            || page.generation != header.generation
            || page.page_index != page_index
        {
            return Err(ManifestError::Corrupt(format!(
                "page {page_index} metadata does not match its manifest"
            )));
        }
        let expected_len = expected_page_len(header, page_index);
        if page.rows.len() != expected_len || page.rows.len() > PAGE_SIZE {
            return Err(ManifestError::Corrupt(format!(
                "page {page_index} has {} rows, expected {expected_len}",
                page.rows.len()
            )));
        }
        if page
            .rows
            .iter()
            .any(|row| !row_belongs_to_scope(row, &header.scope))
        {
            return Err(ManifestError::Corrupt(format!(
                "page {page_index} contains a row outside its scope"
            )));
        }
        for row in &page.rows {
            if !manifest_row_is_bounded(row).map_err(|error| ManifestError::json(&path, error))? {
                return Err(ManifestError::Corrupt(format!(
                    "page {page_index} contains an oversized session row"
                )));
            }
        }
        if page
            .rows
            .windows(2)
            .any(|pair| compare_rows(&pair[0], &pair[1]) == Ordering::Greater)
        {
            return Err(ManifestError::Corrupt(format!(
                "page {page_index} is not in stable recency order"
            )));
        }
        Ok(page.rows)
    }

    /// Validate every immutable page before `current.json` can reference it.
    /// This is linear in the generated cache size but retains only the first
    /// page, the page being checked, and two boundary rows.
    fn validate_generation_pages_in(
        &self,
        header: &ManifestHeader,
        generation_dir: &Path,
        cancel: &AtomicBool,
        on_validation_start: Option<&dyn Fn()>,
    ) -> Result<ManifestPage, ManifestError> {
        if cancel.load(AtomicOrdering::Acquire) {
            return Err(ManifestError::Cancelled);
        }
        if let Some(on_validation_start) = on_validation_start {
            on_validation_start();
        }
        if cancel.load(AtomicOrdering::Acquire) {
            return Err(ManifestError::Cancelled);
        }
        if header.page_count == 0 {
            return Ok(ManifestPage {
                generation: header.generation.clone(),
                page_index: 0,
                total_rows: 0,
                rows: Vec::new(),
                has_next: false,
            });
        }

        let mut first_page_rows = None;
        let mut previous_last: Option<SessionMeta> = None;
        for page_index in 0..header.page_count {
            if cancel.load(AtomicOrdering::Acquire) {
                return Err(ManifestError::Cancelled);
            }
            let rows = self.read_disk_page_from_dir(header, page_index, generation_dir)?;
            if cancel.load(AtomicOrdering::Acquire) {
                return Err(ManifestError::Cancelled);
            }
            if let (Some(previous), Some(current)) = (previous_last.as_ref(), rows.first()) {
                if compare_rows(previous, current) == Ordering::Greater {
                    return Err(ManifestError::Corrupt(format!(
                        "page {page_index} is out of order with the previous page"
                    )));
                }
            }
            previous_last = rows.last().cloned();
            if page_index == 0 {
                first_page_rows = Some(rows);
            }
        }

        let rows = first_page_rows.ok_or_else(|| {
            ManifestError::Corrupt("published manifest has no first page".to_string())
        })?;
        Ok(ManifestPage {
            generation: header.generation.clone(),
            page_index: 0,
            total_rows: header.total_rows,
            rows,
            has_next: header.page_count > 1,
        })
    }

    fn scope_key(&self, scope: &str) -> ScopeKey {
        ScopeKey {
            root: self.root.clone(),
            scope: scope.to_string(),
        }
    }

    fn scope_dir(&self, scope: &str) -> PathBuf {
        self.root.join(scope)
    }

    fn generation_dir(&self, scope: &str, generation: &str) -> PathBuf {
        self.scope_dir(scope).join(generation)
    }

    fn scope_lock_path(&self, scope: &str) -> PathBuf {
        self.scope_dir(scope).join(SCOPE_LOCK_FILE)
    }

    fn coordinator_path(&self, scope: &str) -> PathBuf {
        self.scope_dir(scope).join(COORDINATOR_FILE)
    }

    fn read_floor_path(&self, scope: &str) -> PathBuf {
        self.scope_dir(scope).join(READ_FLOOR_FILE)
    }

    fn read_disk_coordinator_locked(&self, scope: &str) -> Result<DiskCoordinator, ManifestError> {
        let path = self.coordinator_path(scope);
        if !path.exists() {
            return Ok(DiskCoordinator::default());
        }
        let disk: DiskCoordinator = read_json_limited(&path, MAX_COORDINATOR_BYTES)?;
        if disk.format_version != FORMAT_VERSION {
            return Err(ManifestError::Corrupt(format!(
                "unsupported coordinator format in {}",
                path.display()
            )));
        }
        if (!disk.epoch_domain.is_empty() && !valid_epoch_domain(&disk.epoch_domain))
            || disk.min_readable_epoch > disk.build_epoch
            || disk.tombstones.len() > MAX_TOMBSTONES
            || disk.tombstones.iter().any(|item| {
                item.generation > disk.delete_generation
                    || !identity_values_are_bounded(
                        &item.provider_id,
                        &item.session_id,
                        Some(&item.source_path),
                    )
            })
        {
            return Err(ManifestError::Corrupt(format!(
                "coordinator metadata is internally inconsistent in {}",
                path.display()
            )));
        }
        Ok(disk)
    }

    /// Load mutable coordination state under the caller's exclusive scope
    /// lock. Corrupt metadata is a cache failure, not an application failure:
    /// retire every possibly-readable generation, rotate the epoch domain, and
    /// let the authoritative scan publish a clean replacement.
    fn read_or_recover_coordinator_locked(
        &self,
        scope: &str,
    ) -> Result<DiskCoordinator, ManifestError> {
        let coordinator_exists = self.coordinator_path(scope).exists();
        let coordinator = self.read_disk_coordinator_locked(scope);
        let floor = self.read_min_readable_epoch_locked(scope);
        let mut disk = match (coordinator, floor) {
            (Ok(disk), Ok(floor)) => {
                if !coordinator_exists && self.scope_has_cache_artifacts_locked(scope) {
                    return self.recover_scope_metadata_locked(
                        scope,
                        "coordinator is missing while cache artifacts remain",
                    );
                }
                if floor > disk.build_epoch {
                    return self.recover_scope_metadata_locked(
                        scope,
                        "read floor is newer than the coordinator epoch",
                    );
                }
                let mut disk = disk;
                let mut coordinator_changed = false;
                if disk.epoch_domain.is_empty() {
                    disk.epoch_domain = next_epoch_domain();
                    coordinator_changed = true;
                }
                let effective_floor = disk.min_readable_epoch.max(floor);
                if effective_floor != disk.min_readable_epoch {
                    disk.min_readable_epoch = effective_floor;
                    coordinator_changed = true;
                }
                if coordinator_changed || !coordinator_exists {
                    self.write_disk_coordinator_locked(scope, &disk)?;
                }
                if floor != effective_floor {
                    self.write_read_floor_locked(scope, effective_floor)?;
                }
                disk
            }
            (Err(error), _) => {
                return self.recover_scope_metadata_locked(scope, &error.to_string());
            }
            (_, Err(error)) => {
                return self.recover_scope_metadata_locked(scope, &error.to_string());
            }
        };
        if !valid_epoch_domain(&disk.epoch_domain) {
            return self
                .recover_scope_metadata_locked(scope, "coordinator epoch domain is invalid");
        }
        // Keep the in-memory value tied to what was durably written above.
        disk.min_readable_epoch = disk
            .min_readable_epoch
            .max(self.read_min_readable_epoch_locked(scope)?);
        Ok(disk)
    }

    fn scope_has_cache_artifacts_locked(&self, scope: &str) -> bool {
        let scope_dir = self.scope_dir(scope);
        if scope_dir.join(POINTER_FILE).exists() || self.read_floor_path(scope).exists() {
            return true;
        }
        fs::read_dir(scope_dir).ok().is_some_and(|entries| {
            entries.flatten().any(|entry| {
                if !entry.path().is_dir() {
                    return false;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                valid_generation(&name) || artifact_generation(&name, ".building").is_some()
            })
        })
    }

    fn recover_scope_metadata_locked(
        &self,
        scope: &str,
        reason: &str,
    ) -> Result<DiskCoordinator, ManifestError> {
        log::debug!("[SESSION-PAGES] recovering unreadable metadata for {scope}: {reason}");
        let scope_dir = self.scope_dir(scope);
        let recovery_epoch = self.max_known_build_epoch_locked(scope);
        let mut batch = None;
        let mut sequence = 0usize;
        if let Ok(entries) = fs::read_dir(&scope_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if batch.as_ref().is_some_and(|batch| path == *batch) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let retire = name == POINTER_FILE
                    || name == COORDINATOR_FILE
                    || name == READ_FLOOR_FILE
                    || (path.is_dir()
                        && (valid_generation(&name)
                            || artifact_generation(&name, ".building").is_some()
                            || artifact_generation(&name, ".gc").is_some()));
                if retire {
                    quarantine_into_batch(&path, &scope_dir, &mut batch, &mut sequence);
                }
            }
        }

        let mut disk = DiskCoordinator::default();
        disk.build_epoch = recovery_epoch;
        self.write_disk_coordinator_locked(scope, &disk)?;
        self.write_read_floor_locked(scope, 0)?;
        {
            let key = self.scope_key(scope);
            let mut all = coordinator()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = all.scopes.entry(key).or_default();
            if let Some(active) = state.active_cancel.take().and_then(|weak| weak.upgrade()) {
                active.store(true, AtomicOrdering::Release);
            }
        }
        if let Some(batch) = batch {
            remove_artifact_in_background(batch);
        }
        Ok(disk)
    }

    fn max_known_build_epoch_locked(&self, scope: &str) -> u64 {
        let mut maximum = self
            .read_disk_coordinator_locked(scope)
            .ok()
            .map(|disk| disk.build_epoch)
            .unwrap_or(0);
        let mut inspect_header = |path: &Path| {
            let Ok(header) = read_json_limited::<ManifestHeader>(path, MAX_HEADER_BYTES) else {
                return;
            };
            if validate_header(&header, scope).is_ok() {
                maximum = maximum.max(header.build_epoch);
            }
        };
        inspect_header(&self.scope_dir(scope).join(POINTER_FILE));
        if let Ok(entries) = fs::read_dir(self.scope_dir(scope)) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if valid_generation(&name) || artifact_generation(&name, ".building").is_some() {
                    inspect_header(&entry.path().join(HEADER_FILE));
                }
            }
        }
        maximum
    }

    fn write_disk_coordinator_locked(
        &self,
        scope: &str,
        disk: &DiskCoordinator,
    ) -> Result<(), ManifestError> {
        let path = self.coordinator_path(scope);
        write_json_synced(&path, disk)?;
        sync_directory(&self.scope_dir(scope))
    }

    fn read_min_readable_epoch_locked(&self, scope: &str) -> Result<u64, ManifestError> {
        let path = self.read_floor_path(scope);
        if !path.exists() {
            return Ok(0);
        }
        let floor: ReadFloor = read_json_limited(&path, MAX_READ_FLOOR_BYTES)?;
        if floor.format_version != FORMAT_VERSION {
            return Err(ManifestError::Corrupt(format!(
                "unsupported read-floor format in {}",
                path.display()
            )));
        }
        Ok(floor.min_readable_epoch)
    }

    fn write_read_floor_locked(&self, scope: &str, epoch: u64) -> Result<(), ManifestError> {
        write_json_synced(
            &self.read_floor_path(scope),
            &ReadFloor {
                format_version: FORMAT_VERSION,
                min_readable_epoch: epoch,
            },
        )?;
        sync_directory(&self.scope_dir(scope))
    }

    fn record_tombstone_bounded(
        &self,
        scope: &str,
        disk: &mut DiskCoordinator,
        tombstone: Tombstone,
    ) -> Result<(), ManifestError> {
        if let Some(existing) = disk.tombstones.iter_mut().find(|existing| {
            existing.provider_id == tombstone.provider_id
                && existing.session_id == tombstone.session_id
                && existing.source_path == tombstone.source_path
        }) {
            existing.generation = tombstone.generation;
        } else {
            disk.tombstones.push(tombstone);
        }

        let serialized_len = serde_json::to_vec(disk)
            .map_err(|error| ManifestError::json(self.coordinator_path(scope), error))?
            .len() as u64;
        if disk.tombstones.len() > MAX_TOMBSTONES || serialized_len > MAX_COORDINATOR_BYTES / 2 {
            // A fixed-size coordinator must never make future builds/deletes
            // permanently fail. Advancing the readable floor invalidates every
            // older generation (including direct token opens), so exact old
            // identities can be discarded safely. Pre-delete builders also
            // fail their final epoch check; the next fresh build may publish
            // above this floor.
            disk.tombstones.clear();
            disk.min_readable_epoch = disk
                .min_readable_epoch
                .max(self.read_min_readable_epoch_locked(scope)?)
                .max(disk.build_epoch);
            self.write_read_floor_locked(scope, disk.min_readable_epoch)?;
        }
        Ok(())
    }

    fn epoch_is_current(
        &self,
        scope: &str,
        epoch_domain: &str,
        epoch: u64,
    ) -> Result<bool, ManifestError> {
        let _scope_lock = FileLock::shared(&self.scope_lock_path(scope))?;
        let disk = self.read_disk_coordinator_locked(scope)?;
        Ok(disk.epoch_domain == epoch_domain && disk.build_epoch == epoch)
    }

    fn cleanup_scope_artifacts_locked(&self, scope: &str) {
        let scope_dir = self.scope_dir(scope);
        let Ok(entries) = fs::read_dir(&scope_dir) else {
            return;
        };
        let mut batch = None;
        let mut sequence = 0usize;
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            if batch.as_ref().is_some_and(|batch| entry.path() == *batch) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if artifact_generation(&name, ".gc").is_some() {
                quarantine_into_batch(&entry.path(), &scope_dir, &mut batch, &mut sequence);
                continue;
            }
            if artifact_generation(&name, ".building").is_none() {
                continue;
            }
            let owner_path = entry.path().join(BUILD_OWNER_FILE);
            if !owner_path.exists() {
                quarantine_into_batch(&entry.path(), &scope_dir, &mut batch, &mut sequence);
                continue;
            }
            match FileLock::try_exclusive(&owner_path) {
                Ok(Some(owner)) => {
                    drop(owner);
                    quarantine_into_batch(&entry.path(), &scope_dir, &mut batch, &mut sequence);
                }
                Ok(None) => {}
                Err(error) => {
                    log::debug!(
                        "[SESSION-PAGES] failed to inspect stale build {}: {error}",
                        entry.path().display()
                    );
                }
            }
        }
        if let Some(batch) = batch {
            remove_artifact_in_background(batch);
        }
    }

    /// Keep current, all leased generations, and one unleased fallback. Other
    /// immutable generations are first renamed out of the readable namespace
    /// while holding the cross-process scope lock, then removed without the lock.
    fn collect_old_generations(&self, scope: &str, published_generation: &str) {
        let scope_dir = self.scope_dir(scope);
        let retired = {
            let Ok(_scope_lock) = FileLock::exclusive(&self.scope_lock_path(scope)) else {
                return;
            };
            self.cleanup_scope_artifacts_locked(scope);
            // Another process may have published between our pointer update and
            // this best-effort GC. Re-read current while holding the same OS lock
            // used by publication.
            let current = self
                .read_current_header(scope)
                .ok()
                .flatten()
                .map(|header| header.generation)
                .unwrap_or_else(|| published_generation.to_string());
            let fallback = fs::read_dir(&scope_dir).ok().and_then(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        (entry.path().is_dir() && valid_generation(&name) && name != current)
                            .then_some(name)
                    })
                    .max()
            });

            let Ok(entries) = fs::read_dir(&scope_dir) else {
                return;
            };
            let mut batch = None;
            let mut sequence = 0usize;
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let generation = entry.file_name().to_string_lossy().into_owned();
                if !valid_generation(&generation)
                    || generation == current
                    || fallback.as_ref() == Some(&generation)
                {
                    continue;
                }
                let from = entry.path();
                let lease_path = from.join(GENERATION_LEASE_FILE);
                let Ok(Some(lease)) = FileLock::try_exclusive(&lease_path) else {
                    continue;
                };
                quarantine_into_batch(&from, &scope_dir, &mut batch, &mut sequence);
                drop(lease);
            }
            batch
        };
        if let Some(retired) = retired {
            remove_artifact_in_background(retired);
        }
    }
}

/// One-shot CLI builds use a fresh physical namespace for each command.
///
/// A CLI command scans live sources and needs its own immutable reader, but it
/// must not advance the TUI cache's epoch or cancel another command that happens
/// to scan the same scope. The namespace lease keeps its files alive for the
/// returned reader. The last clone atomically retires the namespace; a later
/// CLI invocation also collects crash leftovers whose leases are no longer held.
#[derive(Debug, Clone)]
pub(crate) struct CliManifestStore {
    inner: PagedManifestStore,
}

impl CliManifestStore {
    pub(crate) fn open() -> Result<Self, ManifestError> {
        let config_dir = resolve_config_dir_without_following_user_symlinks(&get_app_config_dir())
            .map_err(|error| ManifestError::Corrupt(error.to_string()))?;
        Self::open_at(&config_dir)
    }

    pub(crate) fn open_at(config_dir: &Path) -> Result<Self, ManifestError> {
        let cli_root = config_dir.join(CLI_ROOT_DIR);
        create_private_dir(&cli_root)?;
        let root_lock = FileLock::exclusive(&cli_root.join(CLI_ROOT_LOCK_FILE))?;
        let namespace = format!("cli-{}", uuid::Uuid::new_v4().simple());
        debug_assert!(valid_cli_namespace(&namespace));
        let retired = cleanup_cli_namespaces_locked(&cli_root, &namespace);

        let namespace_root = cli_root.join(&namespace);
        create_private_dir(&namespace_root)?;
        let namespace_lease =
            Arc::new(FileLock::shared(&namespace_root.join(NAMESPACE_LOCK_FILE))?);
        let cleanup = Arc::new(NamespaceCleanup {
            namespace_parent: cli_root,
            namespace_root: namespace_root.clone(),
            root_lock_file: CLI_ROOT_LOCK_FILE,
            log_target: "SESSION-CLI-PAGES",
            #[cfg(test)]
            deletion_barriers: None,
        });
        let store = Self {
            inner: PagedManifestStore::open_root(
                namespace_root,
                Some(namespace_lease),
                Some(cleanup),
            )?,
        };
        drop(root_lock);
        for path in retired {
            #[cfg(test)]
            remove_namespace_artifact_in_background(path, None);
            #[cfg(not(test))]
            remove_namespace_artifact_in_background(path);
        }
        Ok(store)
    }

    pub(crate) fn begin_build(&self, scope: &str) -> Result<PagedManifestBuilder, ManifestError> {
        self.inner.begin_build(scope)
    }
}

/// Query results live in a physically separate namespace from authoritative
/// session manifests. The wrapper type prevents a filtered build from updating
/// or cancelling the base store's `current.json`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct QueryManifestNamespace(String);

impl QueryManifestNamespace {
    pub(crate) fn new_unique() -> Self {
        Self(format!("tui-{}", uuid::Uuid::new_v4().simple()))
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        let namespace = Self(value.to_string());
        assert!(valid_query_namespace(namespace.as_str()));
        namespace
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub(crate) struct QueryManifestStore {
    inner: PagedManifestStore,
    namespace: QueryManifestNamespace,
}

impl QueryManifestStore {
    pub(crate) fn open(namespace: &QueryManifestNamespace) -> Result<Self, ManifestError> {
        let config_dir = resolve_config_dir_without_following_user_symlinks(&get_app_config_dir())
            .map_err(|error| ManifestError::Corrupt(error.to_string()))?;
        Self::open_at(&config_dir, namespace)
    }

    pub(crate) fn open_at(
        config_dir: &Path,
        namespace: &QueryManifestNamespace,
    ) -> Result<Self, ManifestError> {
        if !valid_query_namespace(namespace.as_str()) {
            return Err(ManifestError::InvalidOptions(
                "invalid session query namespace".to_string(),
            ));
        }
        let query_root = config_dir.join(QUERY_ROOT_DIR);
        create_private_dir(&query_root)?;
        let root_lock = FileLock::exclusive(&query_root.join(QUERY_ROOT_LOCK_FILE))?;
        let retired = cleanup_query_namespaces_locked(&query_root, namespace.as_str());

        let namespace_root = query_root.join(namespace.as_str());
        create_private_dir(&namespace_root)?;
        let namespace_lease =
            Arc::new(FileLock::shared(&namespace_root.join(NAMESPACE_LOCK_FILE))?);
        let cleanup = Arc::new(NamespaceCleanup {
            namespace_parent: query_root,
            namespace_root: namespace_root.clone(),
            root_lock_file: QUERY_ROOT_LOCK_FILE,
            log_target: "SESSION-QUERY-PAGES",
            #[cfg(test)]
            deletion_barriers: None,
        });
        let store = Self {
            inner: PagedManifestStore::open_root(
                namespace_root,
                Some(namespace_lease),
                Some(cleanup),
            )?,
            namespace: namespace.clone(),
        };
        drop(root_lock);
        for path in retired {
            #[cfg(test)]
            remove_namespace_artifact_in_background(path, None);
            #[cfg(not(test))]
            remove_namespace_artifact_in_background(path);
        }
        Ok(store)
    }

    pub(crate) fn namespace(&self) -> &QueryManifestNamespace {
        &self.namespace
    }

    pub(crate) fn begin_build(
        &self,
        base: &ManifestReader,
    ) -> Result<QueryManifestBuilder, ManifestError> {
        Ok(QueryManifestBuilder {
            inner: self.inner.begin_build(base.scope())?,
            base_scope: base.scope().to_string(),
            base_generation: base.generation().to_string(),
        })
    }

    pub(crate) fn open_reader(&self, scope: &str) -> Option<ManifestReader> {
        self.inner.open_reader(scope)
    }

    pub(crate) fn open_generation(&self, scope: &str, generation: &str) -> Option<ManifestReader> {
        self.inner.open_generation(scope, generation)
    }

    pub(crate) fn load_page(&self, scope: &str, page_index: usize) -> Option<ManifestPage> {
        self.inner.load_page(scope, page_index)
    }

    pub(crate) fn cancel(&self, scope: &str) {
        self.inner.cancel(scope);
    }
}

pub(crate) struct QueryManifestBuilder {
    inner: PagedManifestBuilder,
    base_scope: String,
    base_generation: String,
}

impl QueryManifestBuilder {
    pub(crate) fn base_scope(&self) -> &str {
        &self.base_scope
    }

    pub(crate) fn base_generation(&self) -> &str {
        &self.base_generation
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    pub(crate) fn push(&mut self, row: SessionMeta) -> Result<(), ManifestError> {
        self.inner.push(row)
    }

    pub(crate) fn publish(self) -> Result<PublishedManifest, ManifestError> {
        self.inner.publish()
    }

    pub(crate) fn publish_cancellable(
        self,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<PublishedManifest, ManifestError> {
        let cancel = Arc::clone(&self.inner.cancel);
        let finished = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                while !finished.load(AtomicOrdering::Acquire) {
                    if is_cancelled() {
                        cancel.store(true, AtomicOrdering::Release);
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            });
            let result = self.inner.publish();
            finished.store(true, AtomicOrdering::Release);
            result
        })
    }
}

/// A generation-pinned reader. Keeping this value alive prevents background
/// garbage collection from deleting its immutable pages.
#[derive(Debug, Clone)]
pub(crate) struct ManifestReader {
    /// Release the generation lease before the store's last namespace
    /// cleanup guard runs. Rust drops struct fields in declaration order.
    _lease: Arc<FileLock>,
    header: ManifestHeader,
    store: PagedManifestStore,
}

impl ManifestReader {
    pub(crate) fn scope(&self) -> &str {
        &self.header.scope
    }

    pub(crate) fn generation(&self) -> &str {
        &self.header.generation
    }

    pub(crate) const fn build_epoch(&self) -> u64 {
        self.header.build_epoch
    }

    pub(crate) fn total_rows(&self) -> usize {
        self.header.total_rows
    }

    pub(crate) fn page_count(&self) -> usize {
        self.header.page_count
    }

    pub(crate) fn load_page(&self, page_index: usize) -> Option<ManifestPage> {
        self.store.read_page(&self.header, page_index)
    }
}

/// Infallible transformation applied to rows after the identity merge has
/// ordered them, but before duplicate identities and recency are resolved.
///
/// Implementations must not change `provider_id`, `session_id`, or
/// `source_path`: those fields define both the run order and deduplication
/// identity. Enrichers run in registration order and should degrade locally
/// when their optional backing source is unavailable.
pub(crate) trait IdentityRowEnricher: Send {
    fn enrich(&mut self, row: &mut SessionMeta);
}

impl<F> IdentityRowEnricher for F
where
    F: FnMut(&mut SessionMeta) + Send,
{
    fn enrich(&mut self, row: &mut SessionMeta) {
        self(row);
    }
}

/// Streaming manifest builder. The only N-sized state lives in temporary files;
/// the in-memory buffer never exceeds its configured spill bound.
pub(crate) struct PagedManifestBuilder {
    store: PagedManifestStore,
    key: ScopeKey,
    scope: String,
    generation: String,
    epoch_domain: String,
    epoch: u64,
    started_delete_generation: u64,
    cancel: Arc<AtomicBool>,
    options: BuildOptions,
    staging_dir: PathBuf,
    owner_lock: Option<FileLock>,
    buffer: Vec<SessionMeta>,
    run_levels: Vec<Vec<PathBuf>>,
    next_run_id: usize,
    published: bool,
    /// A repack must publish only if it still derives from the exact current
    /// immutable generation captured after its build epoch was won.
    expected_source: Option<(String, u64)>,
    identity_row_enrichers: Vec<Box<dyn IdentityRowEnricher>>,
    #[cfg(test)]
    peak_retained_run_paths: usize,
    /// Deterministic fault injection for the pointer-publication regression
    /// test. Production builds have no corresponding branch or field.
    #[cfg(test)]
    corrupt_page_before_validation: Option<usize>,
    /// Pauses immediately before the staging pages are validated. Tests use
    /// this to prove that validation owns neither the process-global
    /// coordinator mutex nor a scope publication lock.
    #[cfg(test)]
    validation_barriers: Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>,
}

impl PagedManifestBuilder {
    fn bind_source(&mut self, reader: &ManifestReader) -> Result<(), ManifestError> {
        if reader.store.root != self.store.root || reader.scope() != self.scope {
            return Err(ManifestError::InvalidOptions(
                "session manifest repack source belongs to another store or scope".to_string(),
            ));
        }
        self.expected_source = Some((reader.generation().to_string(), reader.build_epoch()));
        Ok(())
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.load(AtomicOrdering::Acquire)
    }

    pub(crate) fn scope(&self) -> &str {
        &self.scope
    }

    /// Register an infallible row enricher for the identity-ordered publish
    /// pass. Registration may happen after rows have been pushed because every
    /// row is enriched only when the completed identity run is consumed.
    pub(crate) fn add_identity_enricher(
        &mut self,
        enricher: Box<dyn IdentityRowEnricher>,
    ) -> Result<(), ManifestError> {
        self.ensure_active()?;
        self.identity_row_enrichers.push(enricher);
        Ok(())
    }

    pub(crate) fn push(&mut self, row: SessionMeta) -> Result<(), ManifestError> {
        self.ensure_active()?;
        if !row_belongs_to_scope(&row, &self.scope) {
            return Err(ManifestError::RowOutsideScope {
                scope: self.scope.clone(),
                provider: row.provider_id,
            });
        }
        let Some(row) = sanitize_manifest_row(row)
            .map_err(|error| ManifestError::json(&self.staging_dir, error))?
        else {
            return Ok(());
        };
        self.buffer.push(row);
        if self.buffer.len() >= self.options.spill_rows {
            self.flush_buffer(RunOrder::Identity)?;
        }
        Ok(())
    }

    pub(crate) fn publish(mut self) -> Result<PublishedManifest, ManifestError> {
        self.ensure_persistent_active()?;
        self.flush_buffer(RunOrder::Identity)?;
        let identity_run = self.finish_runs(RunOrder::Identity)?;
        self.build_deduplicated_recency_runs(identity_run)?;
        let recency_run = self.finish_runs(RunOrder::Recency)?;
        self.ensure_persistent_active()?;

        let tombstones = self.tombstones_for_publish()?;
        let (total_rows, page_count) = self.write_pages(recency_run, &tombstones)?;
        let header = ManifestHeader {
            format_version: FORMAT_VERSION,
            scan_cache_version: super::cache::SCAN_CACHE_VERSION,
            epoch_domain: self.epoch_domain.clone(),
            build_epoch: self.epoch,
            scope: self.scope.clone(),
            generation: self.generation.clone(),
            page_size: PAGE_SIZE,
            total_rows,
            page_count,
        };
        let header_path = self.staging_dir.join(HEADER_FILE);
        write_json_synced(&header_path, &header)?;
        #[cfg(test)]
        if let Some(page_index) = self.corrupt_page_before_validation {
            let path = self.staging_dir.join(page_file_name(page_index));
            fs::write(&path, b"not-json").map_err(|error| ManifestError::io(&path, error))?;
        }
        sync_directory(&self.staging_dir)?;

        // Validate the complete immutable generation while it is still in the
        // owner-leased staging directory. This is intentionally outside both
        // the per-scope publication lock and the process-global cancellation
        // registry: a million-row validation must not stall unrelated scopes.
        let staged_header: ManifestHeader =
            read_json_limited(&self.staging_dir.join(HEADER_FILE), MAX_HEADER_BYTES)?;
        validate_header(&staged_header, &self.scope)?;
        if !valid_generation(&staged_header.generation) {
            return Err(ManifestError::Corrupt(
                "new session manifest has an invalid generation".to_string(),
            ));
        }
        if staged_header != header {
            return Err(ManifestError::Corrupt(
                "new session manifest header changed before validation".to_string(),
            ));
        }
        #[cfg(test)]
        let validation_barriers = self.validation_barriers.clone();
        #[cfg(test)]
        let validation_hook = || {
            if let Some((started, resume)) = validation_barriers.as_ref() {
                started.wait();
                resume.wait();
            }
        };
        #[cfg(test)]
        let validation_hook_ref: Option<&dyn Fn()> = Some(&validation_hook);
        #[cfg(not(test))]
        let validation_hook_ref: Option<&dyn Fn()> = None;
        let first_page = self.store.validate_generation_pages_in(
            &staged_header,
            &self.staging_dir,
            &self.cancel,
            validation_hook_ref,
        )?;
        self.ensure_persistent_active()?;

        let final_dir = self.store.generation_dir(&self.scope, &self.generation);
        let pointer_path = self.store.scope_dir(&self.scope).join(POINTER_FILE);
        let published_reader = {
            let _scope_lock = FileLock::exclusive(&self.store.scope_lock_path(&self.scope))?;
            let mut disk = self.store.read_disk_coordinator_locked(&self.scope)?;
            if self.cancel.load(AtomicOrdering::Acquire)
                || disk.epoch_domain != self.epoch_domain
                || disk.build_epoch != self.epoch
            {
                return Err(ManifestError::Cancelled);
            }
            if let Some((expected_generation, expected_epoch)) = self.expected_source.as_ref() {
                let Some(current) = self.store.read_current_header(&self.scope)? else {
                    return Err(ManifestError::Cancelled);
                };
                if current.generation != *expected_generation
                    || current.build_epoch != *expected_epoch
                {
                    return Err(ManifestError::Cancelled);
                }
            }

            fs::rename(&self.staging_dir, &final_dir)
                .map_err(|error| ManifestError::io(&self.staging_dir, error))?;
            let mut pointer_updated = false;
            let publication = (|| -> Result<ManifestReader, ManifestError> {
                let stored_header: ManifestHeader =
                    read_json_limited(&final_dir.join(HEADER_FILE), MAX_HEADER_BYTES)?;
                if stored_header != header {
                    return Err(ManifestError::Corrupt(
                        "new session manifest header changed before publication".to_string(),
                    ));
                }
                let reader = self.store.lease_reader(stored_header).ok_or_else(|| {
                    ManifestError::Corrupt(
                        "published session manifest could not acquire its reader lease".to_string(),
                    )
                })?;
                write_json_synced(&pointer_path, &header)?;
                pointer_updated = true;

                // Every tombstone that existed at build start was applied
                // while writing pages. A later delete would have advanced the
                // epoch and prevented this critical section from publishing.
                disk.tombstones
                    .retain(|item| item.generation > self.started_delete_generation);
                self.store
                    .write_disk_coordinator_locked(&self.scope, &disk)?;
                sync_directory(&self.store.scope_dir(&self.scope))?;
                Ok(reader)
            })();
            let reader = match publication {
                Ok(reader) => reader,
                Err(error) => {
                    self.owner_lock.take();
                    if !pointer_updated {
                        fs::remove_dir_all(&final_dir)
                            .map_err(|cleanup| ManifestError::io(&final_dir, cleanup))?;
                    } else {
                        // The durable pointer now references this generation;
                        // Drop must not remove it even if a later coordinator
                        // persistence step failed.
                        self.published = true;
                    }
                    return Err(error);
                }
            };
            self.published = true;
            {
                // This mutex guards only process-local cancellation pointers.
                // In particular, it is never held while validating or syncing
                // a generation. Pointer equality prevents a delayed publisher
                // from clearing a newer builder's cancellation registration.
                let mut all = coordinator()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let state = all.scopes.entry(self.key.clone()).or_default();
                if state
                    .active_cancel
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .is_some_and(|active| Arc::ptr_eq(&active, &self.cancel))
                {
                    state.active_cancel = None;
                }
            }
            reader
        };
        self.owner_lock.take();
        self.store
            .collect_old_generations(&self.scope, &self.generation);
        Ok(PublishedManifest {
            generation: self.generation.clone(),
            build_epoch: self.epoch,
            total_rows,
            page_count,
            first_page,
            reader: published_reader,
        })
    }

    fn ensure_active(&self) -> Result<(), ManifestError> {
        if self.cancel.load(AtomicOrdering::Acquire) {
            return Err(ManifestError::Cancelled);
        }
        Ok(())
    }

    fn ensure_persistent_active(&self) -> Result<(), ManifestError> {
        self.ensure_active()?;
        if !self
            .store
            .epoch_is_current(&self.scope, &self.epoch_domain, self.epoch)?
        {
            return Err(ManifestError::Cancelled);
        }
        Ok(())
    }

    fn tombstones_for_publish(&self) -> Result<TombstoneSet, ManifestError> {
        self.ensure_active()?;
        let _scope_lock = FileLock::shared(&self.store.scope_lock_path(&self.scope))?;
        let disk = self.store.read_disk_coordinator_locked(&self.scope)?;
        if disk.epoch_domain != self.epoch_domain || disk.build_epoch != self.epoch {
            return Err(ManifestError::Cancelled);
        }
        Ok(TombstoneSet::from_items(&disk.tombstones))
    }

    fn flush_buffer(&mut self, order: RunOrder) -> Result<(), ManifestError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        self.ensure_persistent_active()?;
        self.buffer
            .sort_by(|left, right| order.compare(left, right));
        let path = self.next_run_path(order.spill_name());
        write_run(&path, self.buffer.drain(..), &self.cancel)?;
        self.add_run(path, order)
    }

    fn add_run(&mut self, path: PathBuf, order: RunOrder) -> Result<(), ManifestError> {
        let mut level = 0usize;
        let mut carry = path;
        loop {
            if self.run_levels.len() <= level {
                self.run_levels.push(Vec::new());
            }
            self.run_levels[level].push(carry);
            self.record_retained_run_paths();
            if self.run_levels[level].len() < self.options.merge_fan_in {
                return Ok(());
            }
            self.ensure_persistent_active()?;
            let inputs = std::mem::take(&mut self.run_levels[level]);
            let output = self.next_run_path(&format!("{}-level-{level}", order.merge_name()));
            merge_runs(&inputs, &output, &self.cancel, order)?;
            for input in inputs {
                let _ = fs::remove_file(input);
            }
            carry = output;
            level = level.saturating_add(1);
        }
    }

    fn finish_runs(&mut self, order: RunOrder) -> Result<Option<PathBuf>, ManifestError> {
        let mut runs: Vec<PathBuf> = self.run_levels.drain(..).flatten().collect();
        let mut pass = 0usize;
        while runs.len() > 1 {
            self.ensure_persistent_active()?;
            let mut next = Vec::new();
            let mut current = runs.into_iter();
            loop {
                let group: Vec<PathBuf> =
                    current.by_ref().take(self.options.merge_fan_in).collect();
                if group.is_empty() {
                    break;
                }
                if group.len() == 1 {
                    next.push(group.into_iter().next().expect("one run"));
                    continue;
                }
                let output = self.next_run_path(&format!("{}-final-{pass}", order.merge_name()));
                merge_runs(&group, &output, &self.cancel, order)?;
                for input in group {
                    let _ = fs::remove_file(&input);
                }
                next.push(output);
            }
            runs = next;
            pass = pass.saturating_add(1);
        }
        Ok(runs.pop())
    }

    fn build_deduplicated_recency_runs(
        &mut self,
        identity_run: Option<PathBuf>,
    ) -> Result<(), ManifestError> {
        let Some(run_path) = identity_run else {
            return Ok(());
        };
        let mut reader = RunReader::open(&run_path)?;
        let mut pending: Option<SessionMeta> = None;
        let mut visited = 0usize;
        while let Some(mut row) = reader.next_row()? {
            if visited.is_multiple_of(1_024) {
                self.ensure_persistent_active()?;
            }
            visited = visited.saturating_add(1);
            if !self.identity_row_enrichers.is_empty() {
                for enricher in &mut self.identity_row_enrichers {
                    enricher.enrich(&mut row);
                }
                let Some(sanitized) = sanitize_manifest_row(row)
                    .map_err(|error| ManifestError::json(&self.staging_dir, error))?
                else {
                    continue;
                };
                row = sanitized;
            }
            if pending
                .as_ref()
                .is_some_and(|previous| same_identity(previous, &row))
            {
                continue;
            }
            if let Some(previous) = pending.replace(row) {
                self.push_recency_candidate(previous)?;
            }
        }
        if let Some(last) = pending {
            self.push_recency_candidate(last)?;
        }
        drop(reader);
        let _ = fs::remove_file(run_path);
        self.flush_buffer(RunOrder::Recency)
    }

    fn push_recency_candidate(&mut self, row: SessionMeta) -> Result<(), ManifestError> {
        self.buffer.push(row);
        if self.buffer.len() >= self.options.spill_rows {
            self.flush_buffer(RunOrder::Recency)?;
        }
        Ok(())
    }

    fn write_pages(
        &mut self,
        run_path: Option<PathBuf>,
        tombstones: &TombstoneSet,
    ) -> Result<(usize, usize), ManifestError> {
        let Some(run_path) = run_path else {
            return Ok((0, 0));
        };
        let mut reader = RunReader::open(&run_path)?;
        let mut page_rows = Vec::with_capacity(PAGE_SIZE);
        let mut total_rows = 0usize;
        let mut page_index = 0usize;
        let mut previous: Option<SessionMeta> = None;
        let mut visited = 0usize;
        while let Some(row) = reader.next_row()? {
            if visited.is_multiple_of(1_024) {
                self.ensure_persistent_active()?;
            }
            visited = visited.saturating_add(1);
            if tombstones.contains(&row) {
                continue;
            }
            if previous
                .as_ref()
                .is_some_and(|prior| compare_rows(prior, &row) == Ordering::Greater)
            {
                return Err(ManifestError::Corrupt(
                    "external merge produced an out-of-order row".to_string(),
                ));
            }
            previous = Some(row.clone());
            page_rows.push(row);
            total_rows = total_rows.saturating_add(1);
            if page_rows.len() == PAGE_SIZE {
                self.write_page(page_index, std::mem::take(&mut page_rows))?;
                page_rows = Vec::with_capacity(PAGE_SIZE);
                page_index = page_index.saturating_add(1);
            }
        }
        if !page_rows.is_empty() {
            self.write_page(page_index, page_rows)?;
            page_index = page_index.saturating_add(1);
        }
        let _ = fs::remove_file(run_path);
        Ok((total_rows, page_index))
    }

    fn write_page(&self, page_index: usize, rows: Vec<SessionMeta>) -> Result<(), ManifestError> {
        self.ensure_active()?;
        let path = self.staging_dir.join(page_file_name(page_index));
        if rows.len() > PAGE_SIZE {
            return Err(ManifestError::Corrupt(format!(
                "page {page_index} contains more than {PAGE_SIZE} rows"
            )));
        }
        for row in &rows {
            if !manifest_row_is_bounded(row).map_err(|error| ManifestError::json(&path, error))? {
                return Err(ManifestError::Corrupt(format!(
                    "page {page_index} contains an oversized session row"
                )));
            }
        }
        let page = DiskPage {
            format_version: FORMAT_VERSION,
            scan_cache_version: super::cache::SCAN_CACHE_VERSION,
            scope: self.scope.clone(),
            generation: self.generation.clone(),
            page_index,
            rows,
        };
        let bytes =
            serde_json::to_vec_pretty(&page).map_err(|error| ManifestError::json(&path, error))?;
        if bytes.len() as u64 > MAX_PAGE_BYTES {
            return Err(ManifestError::Corrupt(format!(
                "page {page_index} exceeds the cache write limit"
            )));
        }
        write_bytes_synced(&path, &bytes)
    }

    fn next_run_path(&mut self, kind: &str) -> PathBuf {
        let id = self.next_run_id;
        self.next_run_id = self.next_run_id.saturating_add(1);
        self.staging_dir.join(format!("{kind}-{id:08}.jsonl"))
    }

    fn record_retained_run_paths(&mut self) {
        #[cfg(test)]
        {
            self.peak_retained_run_paths = self
                .peak_retained_run_paths
                .max(self.run_levels.iter().map(Vec::len).sum());
        }
    }

    #[cfg(test)]
    fn peak_retained_run_paths(&self) -> usize {
        self.peak_retained_run_paths
    }
}

impl Drop for PagedManifestBuilder {
    fn drop(&mut self) {
        if !self.published {
            self.cancel.store(true, AtomicOrdering::Release);
            self.owner_lock.take();
            let _ = fs::remove_dir_all(&self.staging_dir);
        }
    }
}

/// Bounded top-K helper for a first-ever run with no manifest. A provider's
/// incremental directory walker can feed rows here and emit provisional paints
/// immediately. The result is exact only after that walker reaches EOF: because
/// true recency lives inside transcript content, no implementation can know the
/// exact global top 101 before inspecting all candidates when no prior metadata
/// index exists.
pub(crate) struct BoundedRecencyPreview {
    limit: usize,
    heap: BinaryHeap<PreviewItem>,
}

impl BoundedRecencyPreview {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            heap: BinaryHeap::with_capacity(limit),
        }
    }

    pub(crate) fn push(&mut self, row: SessionMeta) {
        if self.limit == 0 {
            return;
        }
        let Ok(Some(row)) = sanitize_manifest_row(row) else {
            return;
        };
        if self.heap.len() < self.limit {
            self.heap.push(PreviewItem(row));
            return;
        }
        if self
            .heap
            .peek()
            .is_some_and(|oldest| compare_rows(&row, &oldest.0) == Ordering::Less)
        {
            let _ = self.heap.pop();
            self.heap.push(PreviewItem(row));
        }
    }

    pub(crate) fn into_sorted(self) -> Vec<SessionMeta> {
        let mut rows: Vec<_> = self.heap.into_iter().map(|item| item.0).collect();
        rows.sort_by(compare_rows);
        rows
    }

    /// Clone the bounded provisional page without consuming the accumulator.
    /// Callers throttle this operation; it never clones more than `limit` rows.
    pub(crate) fn snapshot(&self) -> Vec<SessionMeta> {
        let mut rows: Vec<_> = self.heap.iter().map(|item| item.0.clone()).collect();
        rows.sort_by(compare_rows);
        rows
    }
}

struct PreviewItem(SessionMeta);

impl PartialEq for PreviewItem {
    fn eq(&self, other: &Self) -> bool {
        compare_rows(&self.0, &other.0) == Ordering::Equal
    }
}

impl Eq for PreviewItem {}

impl PartialOrd for PreviewItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PreviewItem {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_rows(&self.0, &other.0)
    }
}

struct RunReader {
    path: PathBuf,
    reader: BufReader<File>,
    line: String,
}

impl RunReader {
    fn open(path: &Path) -> Result<Self, ManifestError> {
        let file = File::open(path).map_err(|error| ManifestError::io(path, error))?;
        Ok(Self {
            path: path.to_path_buf(),
            reader: BufReader::new(file),
            line: String::new(),
        })
    }

    fn next_row(&mut self) -> Result<Option<SessionMeta>, ManifestError> {
        self.line.clear();
        let max_line_bytes = MAX_SESSION_META_JSON_BYTES.saturating_add(1);
        let bytes = (&mut self.reader)
            .take(max_line_bytes.saturating_add(1) as u64)
            .read_line(&mut self.line)
            .map_err(|error| ManifestError::io(&self.path, error))?;
        if bytes == 0 {
            return Ok(None);
        }
        if bytes > max_line_bytes || !self.line.ends_with('\n') {
            return Err(ManifestError::Corrupt(format!(
                "{} contains an oversized or incomplete session row",
                self.path.display()
            )));
        }
        let row: SessionMeta = serde_json::from_str(self.line.trim_end())
            .map_err(|error| ManifestError::json(&self.path, error))?;
        if !manifest_row_is_bounded(&row).map_err(|error| ManifestError::json(&self.path, error))? {
            return Err(ManifestError::Corrupt(format!(
                "{} contains an oversized session row",
                self.path.display()
            )));
        }
        Ok(Some(row))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunOrder {
    Identity,
    Recency,
}

impl RunOrder {
    fn compare(self, left: &SessionMeta, right: &SessionMeta) -> Ordering {
        match self {
            Self::Identity => compare_identity_then_rows(left, right),
            Self::Recency => compare_rows(left, right),
        }
    }

    const fn spill_name(self) -> &'static str {
        match self {
            Self::Identity => "identity-spill",
            Self::Recency => "recency-spill",
        }
    }

    const fn merge_name(self) -> &'static str {
        match self {
            Self::Identity => "identity-merge",
            Self::Recency => "recency-merge",
        }
    }
}

struct MergeItem {
    source: usize,
    row: SessionMeta,
    order: RunOrder,
}

impl PartialEq for MergeItem {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
            && self.order == other.order
            && self.order.compare(&self.row, &other.row) == Ordering::Equal
    }
}

impl Eq for MergeItem {}

impl PartialOrd for MergeItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MergeItem {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap; reverse the row order so the newest row is
        // popped first. Source index is a deterministic final tie-breaker.
        self.order
            .compare(&other.row, &self.row)
            .then_with(|| other.source.cmp(&self.source))
    }
}

fn merge_runs(
    inputs: &[PathBuf],
    output: &Path,
    cancel: &AtomicBool,
    order: RunOrder,
) -> Result<(), ManifestError> {
    let mut readers: Vec<RunReader> = inputs
        .iter()
        .map(|path| RunReader::open(path))
        .collect::<Result<_, _>>()?;
    let file = create_private_file(output)?;
    let mut writer = BufWriter::new(file);
    let mut heap = BinaryHeap::with_capacity(readers.len());
    for (source, reader) in readers.iter_mut().enumerate() {
        if let Some(row) = reader.next_row()? {
            heap.push(MergeItem { source, row, order });
        }
    }
    let mut emitted = 0usize;
    while let Some(item) = heap.pop() {
        if emitted.is_multiple_of(64) && cancel.load(AtomicOrdering::Acquire) {
            return Err(ManifestError::Cancelled);
        }
        serde_json::to_writer(&mut writer, &item.row)
            .map_err(|error| ManifestError::json(output, error))?;
        writer
            .write_all(b"\n")
            .map_err(|error| ManifestError::io(output, error))?;
        emitted = emitted.saturating_add(1);
        if let Some(row) = readers[item.source].next_row()? {
            heap.push(MergeItem {
                source: item.source,
                row,
                order,
            });
        }
    }
    let file = writer
        .into_inner()
        .map_err(|error| ManifestError::io(output, error.into_error()))?;
    file.sync_all()
        .map_err(|error| ManifestError::io(output, error))
}

fn write_run(
    path: &Path,
    rows: impl Iterator<Item = SessionMeta>,
    cancel: &AtomicBool,
) -> Result<(), ManifestError> {
    let file = create_private_file(path)?;
    let mut writer = BufWriter::new(file);
    for (index, row) in rows.enumerate() {
        if index % 64 == 0 && cancel.load(AtomicOrdering::Acquire) {
            return Err(ManifestError::Cancelled);
        }
        serde_json::to_writer(&mut writer, &row)
            .map_err(|error| ManifestError::json(path, error))?;
        writer
            .write_all(b"\n")
            .map_err(|error| ManifestError::io(path, error))?;
    }
    let file = writer
        .into_inner()
        .map_err(|error| ManifestError::io(path, error.into_error()))?;
    file.sync_all()
        .map_err(|error| ManifestError::io(path, error))
}

fn identity_values_are_bounded(
    provider_id: &str,
    session_id: &str,
    source_path: Option<&str>,
) -> bool {
    provider_id.len() <= MAX_IDENTITY_FIELD_BYTES
        && session_id.len() <= MAX_IDENTITY_FIELD_BYTES
        && source_path.is_none_or(|path| path.len() <= MAX_IDENTITY_FIELD_BYTES)
}

fn optional_field_is_bounded(value: Option<&str>, max_bytes: usize) -> bool {
    value.is_none_or(|value| value.len() <= max_bytes)
}

fn serialized_session_meta_len(row: &SessionMeta) -> Result<usize, serde_json::Error> {
    serde_json::to_vec(row).map(|bytes| bytes.len())
}

fn manifest_row_is_bounded(row: &SessionMeta) -> Result<bool, serde_json::Error> {
    if !identity_values_are_bounded(
        &row.provider_id,
        &row.session_id,
        row.source_path.as_deref(),
    ) || !optional_field_is_bounded(row.title.as_deref(), MAX_DISPLAY_FIELD_BYTES)
        || !optional_field_is_bounded(row.summary.as_deref(), MAX_DISPLAY_FIELD_BYTES)
        || !optional_field_is_bounded(row.project_dir.as_deref(), MAX_DISPLAY_FIELD_BYTES)
        || !optional_field_is_bounded(row.resume_command.as_deref(), MAX_DISPLAY_FIELD_BYTES)
    {
        return Ok(false);
    }
    Ok(serialized_session_meta_len(row)? <= MAX_SESSION_META_JSON_BYTES)
}

fn truncate_display_field(value: &mut Option<String>) {
    if let Some(value) = value.as_mut() {
        super::truncate_string_utf8(value, MAX_DISPLAY_FIELD_BYTES);
        normalize_string_capacity(value, MAX_DISPLAY_FIELD_BYTES);
    }
}

fn normalize_action_field(value: &mut Option<String>) {
    if value
        .as_ref()
        .is_some_and(|value| value.len() > MAX_DISPLAY_FIELD_BYTES)
    {
        *value = None;
    } else if let Some(value) = value.as_mut() {
        normalize_string_capacity(value, MAX_DISPLAY_FIELD_BYTES);
    }
}

fn normalize_string_capacity(value: &mut String, max_capacity: usize) {
    if value.capacity() > max_capacity {
        *value = value.as_str().to_owned();
    }
}

/// Normalize a row before it enters either an in-memory preview or a spill
/// file. Identity fields and actionable command/path values are all-or-nothing;
/// only display-only title and summary fields may be truncated.
pub(super) fn sanitize_manifest_row(
    mut row: SessionMeta,
) -> Result<Option<SessionMeta>, serde_json::Error> {
    if !identity_values_are_bounded(
        &row.provider_id,
        &row.session_id,
        row.source_path.as_deref(),
    ) {
        return Ok(None);
    }

    normalize_string_capacity(&mut row.provider_id, MAX_IDENTITY_FIELD_BYTES);
    normalize_string_capacity(&mut row.session_id, MAX_IDENTITY_FIELD_BYTES);
    if let Some(source_path) = row.source_path.as_mut() {
        normalize_string_capacity(source_path, MAX_IDENTITY_FIELD_BYTES);
    }

    truncate_display_field(&mut row.title);
    truncate_display_field(&mut row.summary);
    normalize_action_field(&mut row.project_dir);
    normalize_action_field(&mut row.resume_command);
    if serialized_session_meta_len(&row)? <= MAX_SESSION_META_JSON_BYTES {
        return Ok(Some(row));
    }

    // Escaped JSON can be larger than the UTF-8 input. Drop optional fields in
    // least-useful-first order until the exact compact JSON fits.
    row.resume_command = None;
    if serialized_session_meta_len(&row)? <= MAX_SESSION_META_JSON_BYTES {
        return Ok(Some(row));
    }
    row.summary = None;
    if serialized_session_meta_len(&row)? <= MAX_SESSION_META_JSON_BYTES {
        return Ok(Some(row));
    }
    row.project_dir = None;
    if serialized_session_meta_len(&row)? <= MAX_SESSION_META_JSON_BYTES {
        return Ok(Some(row));
    }
    row.title = None;
    if serialized_session_meta_len(&row)? <= MAX_SESSION_META_JSON_BYTES {
        return Ok(Some(row));
    }

    // The remaining bytes belong to the identity. Truncating them would merge
    // or delete the wrong session, so omit the row from this disposable cache.
    Ok(None)
}

fn compare_rows(a: &SessionMeta, b: &SessionMeta) -> Ordering {
    let a_recency = a.last_active_at.or(a.created_at).unwrap_or(0);
    let b_recency = b.last_active_at.or(b.created_at).unwrap_or(0);
    b_recency
        .cmp(&a_recency)
        .then_with(|| a.provider_id.cmp(&b.provider_id))
        .then_with(|| a.session_id.cmp(&b.session_id))
        .then_with(|| a.source_path.cmp(&b.source_path))
        .then_with(|| a.created_at.cmp(&b.created_at))
        .then_with(|| a.last_active_at.cmp(&b.last_active_at))
        .then_with(|| a.title.cmp(&b.title))
        .then_with(|| a.summary.cmp(&b.summary))
        .then_with(|| a.project_dir.cmp(&b.project_dir))
        .then_with(|| a.resume_command.cmp(&b.resume_command))
}

fn compare_identity_then_rows(a: &SessionMeta, b: &SessionMeta) -> Ordering {
    a.provider_id
        .cmp(&b.provider_id)
        .then_with(|| a.session_id.cmp(&b.session_id))
        .then_with(|| {
            a.source_path
                .as_deref()
                .unwrap_or_default()
                .cmp(b.source_path.as_deref().unwrap_or_default())
        })
        .then_with(|| compare_rows(a, b))
}

fn same_identity(a: &SessionMeta, b: &SessionMeta) -> bool {
    a.provider_id == b.provider_id
        && a.session_id == b.session_id
        && a.source_path.as_deref().unwrap_or_default()
            == b.source_path.as_deref().unwrap_or_default()
}

fn validate_scope(scope: &str) -> Result<(), ManifestError> {
    if scope == "all" || CACHED_PROVIDERS.contains(&scope) {
        Ok(())
    } else {
        Err(ManifestError::UnsupportedScope(scope.to_string()))
    }
}

fn row_belongs_to_scope(row: &SessionMeta, scope: &str) -> bool {
    CACHED_PROVIDERS.contains(&row.provider_id.as_str())
        && (scope == "all" || row.provider_id == scope)
}

fn validate_header(header: &ManifestHeader, scope: &str) -> Result<(), ManifestError> {
    if header.format_version != FORMAT_VERSION
        || header.scan_cache_version != super::cache::SCAN_CACHE_VERSION
        || !valid_epoch_domain(&header.epoch_domain)
        || header.scope != scope
        || header.page_size != PAGE_SIZE
        || header.page_count != header.total_rows.div_ceil(PAGE_SIZE)
    {
        return Err(ManifestError::Corrupt(
            "manifest header is incompatible or internally inconsistent".to_string(),
        ));
    }
    Ok(())
}

fn expected_page_len(header: &ManifestHeader, page_index: usize) -> usize {
    let consumed = page_index.saturating_mul(PAGE_SIZE);
    header.total_rows.saturating_sub(consumed).min(PAGE_SIZE)
}

fn page_file_name(page_index: usize) -> String {
    format!("page-{page_index:08}.json")
}

fn valid_generation(generation: &str) -> bool {
    generation.starts_with("gen-")
        && generation.len() <= 96
        && generation
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_query_namespace(namespace: &str) -> bool {
    namespace.starts_with("tui-")
        && namespace.len() <= 96
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_cli_namespace(namespace: &str) -> bool {
    namespace.starts_with("cli-")
        && namespace.len() <= 96
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn cleanup_cli_namespaces_locked(cli_root: &Path, active_namespace: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(cli_root) else {
        return Vec::new();
    };
    let mut retired = Vec::new();
    let mut batch = None;
    let mut sequence = 0usize;
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == active_namespace {
            continue;
        }
        if artifact_generation(&name, ".gc").is_some() {
            retired.push(entry.path());
            continue;
        }
        if !valid_cli_namespace(&name) {
            continue;
        }
        let lease_path = entry.path().join(NAMESPACE_LOCK_FILE);
        match FileLock::try_exclusive(&lease_path) {
            Ok(Some(lease)) => {
                quarantine_into_batch(&entry.path(), cli_root, &mut batch, &mut sequence);
                drop(lease);
            }
            Ok(None) => {}
            Err(error) => log::debug!(
                "[SESSION-CLI-PAGES] failed to inspect namespace {}: {error}",
                entry.path().display()
            ),
        }
    }
    if let Some(batch) = batch {
        retired.push(batch);
    }
    retired
}

fn cleanup_query_namespaces_locked(query_root: &Path, active_namespace: &str) -> Vec<PathBuf> {
    cleanup_leased_namespaces_locked(
        query_root,
        active_namespace,
        valid_query_namespace,
        "SESSION-QUERY-PAGES",
    )
}

/// Each TUI owns a physical query namespace. A shared lease follows every
/// reader. Opening another namespace quarantines only siblings whose leases
/// were released by completed or crashed processes.
fn cleanup_leased_namespaces_locked(
    namespace_root: &Path,
    active_namespace: &str,
    valid_namespace: fn(&str) -> bool,
    log_target: &str,
) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(namespace_root) else {
        return Vec::new();
    };
    let mut retired = Vec::new();
    let mut batch = None;
    let mut sequence = 0usize;
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path() == namespace_root.join(active_namespace) {
            continue;
        }
        if artifact_generation(&name, ".gc").is_some() {
            retired.push(entry.path());
            continue;
        }
        if !valid_namespace(&name) {
            continue;
        }
        let lease_path = entry.path().join(NAMESPACE_LOCK_FILE);
        match FileLock::try_exclusive(&lease_path) {
            Ok(Some(lease)) => {
                quarantine_into_batch(&entry.path(), namespace_root, &mut batch, &mut sequence);
                drop(lease);
            }
            Ok(None) => {}
            Err(error) => log::debug!(
                "[{log_target}] failed to inspect namespace {}: {error}",
                entry.path().display()
            ),
        }
    }
    if let Some(batch) = batch {
        retired.push(batch);
    }
    retired
}

fn artifact_generation<'a>(name: &'a str, suffix: &str) -> Option<&'a str> {
    let generation = name.strip_prefix('.')?.strip_suffix(suffix)?;
    valid_generation(generation).then_some(generation)
}

fn quarantine_into_batch(
    path: &Path,
    scope_dir: &Path,
    batch: &mut Option<PathBuf>,
    sequence: &mut usize,
) {
    if batch.is_none() {
        let candidate = scope_dir.join(format!(".{}.gc", next_generation()));
        if create_private_dir(&candidate).is_err() {
            return;
        }
        *batch = Some(candidate);
    }
    let Some(batch_path) = batch.as_ref() else {
        return;
    };
    let target = batch_path.join(format!("item-{sequence:08}"));
    if fs::rename(path, target).is_ok() {
        *sequence = sequence.saturating_add(1);
    }
}

fn remove_artifact_in_background(path: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("cc-switch-session-page-gc".to_string())
        .spawn(move || {
            let _ = fs::remove_dir_all(path);
        });
}

#[cfg(not(test))]
fn remove_namespace_artifact_in_background(path: PathBuf) {
    remove_artifact_in_background(path);
}

#[cfg(test)]
fn remove_namespace_artifact_in_background(
    path: PathBuf,
    barriers: Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>,
) {
    let _ = std::thread::Builder::new()
        .name("cc-switch-session-cli-page-gc".to_string())
        .spawn(move || {
            if let Some((started, resume)) = barriers {
                started.wait();
                resume.wait();
            }
            let _ = fs::remove_dir_all(path);
        });
}

fn next_epoch(current: u64) -> Result<u64, ManifestError> {
    current.checked_add(1).ok_or_else(|| {
        ManifestError::Corrupt("session manifest coordinator epoch overflowed".to_string())
    })
}

fn next_epoch_domain() -> String {
    format!("domain-{}", uuid::Uuid::new_v4().simple())
}

fn valid_epoch_domain(domain: &str) -> bool {
    domain.starts_with("domain-")
        && domain.len() <= 96
        && domain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn next_generation() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    format!(
        "gen-{nanos:032x}-{:08x}-{sequence:016x}",
        std::process::id()
    )
}

fn create_private_dir(path: &Path) -> Result<(), ManifestError> {
    fs::create_dir_all(path).map_err(|error| ManifestError::io(path, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| ManifestError::io(path, error))?;
    }
    Ok(())
}

fn create_private_file(path: &Path) -> Result<File, ManifestError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| ManifestError::io(path, error))
}

fn open_lock_file(path: &Path) -> Result<File, ManifestError> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .map_err(|error| ManifestError::io(path, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| ManifestError::io(path, error))?;
    }
    Ok(file)
}

fn write_json_synced<T: Serialize>(path: &Path, value: &T) -> Result<(), ManifestError> {
    write_json_file(path, value).map_err(|error| ManifestError::Corrupt(error.to_string()))?;
    sync_private_file(path)
}

fn write_bytes_synced(path: &Path, bytes: &[u8]) -> Result<(), ManifestError> {
    atomic_write(path, bytes).map_err(|error| ManifestError::Corrupt(error.to_string()))?;
    sync_private_file(path)
}

fn sync_private_file(path: &Path) -> Result<(), ManifestError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| ManifestError::io(path, error))?;
    }
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| ManifestError::io(path, error))
}

fn sync_directory(path: &Path) -> Result<(), ManifestError> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|dir| dir.sync_all())
            .map_err(|error| ManifestError::io(path, error))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn read_json_limited<T: DeserializeOwned>(path: &Path, max_bytes: u64) -> Result<T, ManifestError> {
    let file = File::open(path).map_err(|error| ManifestError::io(path, error))?;
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| ManifestError::io(path, error))?;
    if bytes.len() as u64 > max_bytes {
        return Err(ManifestError::Corrupt(format!(
            "{} exceeds the cache read limit",
            path.display()
        )));
    }
    serde_json::from_slice(&bytes).map_err(|error| ManifestError::json(path, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn meta(provider: &str, id: &str, recency: i64) -> SessionMeta {
        SessionMeta {
            provider_id: provider.to_string(),
            session_id: id.to_string(),
            title: Some(format!("title {id}")),
            created_at: Some(recency.saturating_sub(1)),
            last_active_at: Some(recency),
            source_path: Some(format!("/{provider}/{id}.jsonl")),
            ..SessionMeta::default()
        }
    }

    fn test_store() -> (tempfile::TempDir, PagedManifestStore) {
        let temp = tempdir().expect("tempdir");
        let store = PagedManifestStore::open_at(temp.path()).expect("open store");
        (temp, store)
    }

    fn forget_process_local_scope(store: &PagedManifestStore, scope: &str) {
        coordinator()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .scopes
            .remove(&store.scope_key(scope));
    }

    #[test]
    fn publishes_fixed_pages_with_exact_has_next() {
        let (_temp, store) = test_store();
        let mut builder = store.begin_build("all").expect("begin");
        for index in 0..205 {
            builder
                .push(meta("claude", &format!("s-{index:03}"), index))
                .expect("push");
        }
        let published = builder.publish().expect("publish");

        assert_eq!(published.total_rows, 205);
        assert_eq!(published.page_count, 3);
        assert_eq!(published.first_page.rows.len(), PAGE_SIZE);
        assert_eq!(published.first_page.rows[0].session_id, "s-204");
        assert_eq!(published.first_page.rows[99].session_id, "s-105");
        assert!(published.first_page.has_next);

        let reader = store.open_reader("all").expect("reader");
        let second = reader.load_page(1).expect("page 1");
        assert_eq!(second.rows.len(), 100);
        assert_eq!(second.rows[0].session_id, "s-104");
        assert!(second.has_next);
        let last = reader.load_page(2).expect("page 2");
        assert_eq!(last.rows.len(), 5);
        assert!(!last.has_next);
    }

    #[test]
    fn empty_generation_has_a_readable_empty_first_page() {
        let (_temp, store) = test_store();
        let published = store
            .begin_build("claude")
            .expect("begin")
            .publish()
            .expect("publish");

        assert_eq!(published.total_rows, 0);
        assert_eq!(published.page_count, 0);
        assert!(published.first_page.rows.is_empty());
        let loaded = store.load_page("claude", 0).expect("empty page");
        assert!(loaded.rows.is_empty());
        assert!(!loaded.has_next);
    }

    #[test]
    fn oversized_fields_are_safely_bounded_before_a_full_page_is_written() {
        let (_temp, store) = test_store();
        let mut builder = store.begin_build("claude").expect("begin");
        for index in 0..PAGE_SIZE {
            let mut row = meta("claude", &format!("large-{index:03}"), index as i64);
            row.project_dir = Some("p".repeat(90 * 1024));
            if index == PAGE_SIZE - 1 {
                row.title = Some("界".repeat(MAX_DISPLAY_FIELD_BYTES));
            }
            builder.push(row).expect("push bounded row");
        }
        let published = builder.publish().expect("publish");

        assert_eq!(published.total_rows, PAGE_SIZE);
        assert_eq!(published.first_page.rows.len(), PAGE_SIZE);
        assert!(published.first_page.rows.iter().all(|row| {
            row.project_dir.is_none() && manifest_row_is_bounded(row).expect("serialize row")
        }));
        let utf8_title = published.first_page.rows[0]
            .title
            .as_deref()
            .expect("UTF-8 title");
        assert_eq!(
            utf8_title.len(),
            MAX_DISPLAY_FIELD_BYTES - (MAX_DISPLAY_FIELD_BYTES % "界".len())
        );
        assert!(utf8_title.chars().all(|character| character == '界'));
        let page_path = store
            .generation_dir("claude", &published.generation)
            .join(page_file_name(0));
        assert!(
            fs::metadata(page_path).expect("page metadata").len() <= MAX_PAGE_BYTES,
            "a full page must remain readable by the same hard byte limit"
        );
    }

    #[test]
    fn oversized_command_and_cwd_are_removed_instead_of_truncated() {
        let (_temp, store) = test_store();
        let mut row = meta("claude", "actionable", 1);
        row.resume_command = Some(format!(
            "safe-prefix {}",
            "x".repeat(MAX_DISPLAY_FIELD_BYTES)
        ));
        row.project_dir = Some(format!(
            "/safe-prefix/{}",
            "p".repeat(MAX_DISPLAY_FIELD_BYTES)
        ));

        let mut builder = store.begin_build("claude").expect("begin");
        builder.push(row).expect("push bounded row");
        let published = builder.publish().expect("publish");
        let stored = &published.first_page.rows[0];

        assert!(stored.resume_command.is_none());
        assert!(stored.project_dir.is_none());
    }

    #[test]
    fn oversized_identity_is_skipped_without_truncating_or_merging_it() {
        let (_temp, store) = test_store();
        let oversized_session_id = "s".repeat(MAX_IDENTITY_FIELD_BYTES + 1);
        let oversized_source_path = format!("/{}", "p".repeat(MAX_IDENTITY_FIELD_BYTES));
        assert!(
            sanitize_manifest_row(meta("claude", &oversized_session_id, 3))
                .expect("sanitize session id")
                .is_none()
        );

        let mut oversized_path = meta("claude", "oversized-path", 2);
        oversized_path.source_path = Some(oversized_source_path);
        assert!(sanitize_manifest_row(oversized_path.clone())
            .expect("sanitize source path")
            .is_none());

        let mut builder = store.begin_build("claude").expect("begin");
        builder
            .push(meta("claude", &oversized_session_id, 3))
            .expect("skip oversized session id");
        builder
            .push(oversized_path)
            .expect("skip oversized source path");
        builder
            .push(meta("claude", "kept-exactly", 1))
            .expect("push valid row");
        let published = builder.publish().expect("publish");

        assert_eq!(published.total_rows, 1);
        assert_eq!(published.first_page.rows[0].session_id, "kept-exactly");
    }

    #[test]
    fn unreadable_new_generation_does_not_replace_current_pointer() {
        let (_temp, store) = test_store();
        let mut initial = store.begin_build("claude").expect("initial");
        initial
            .push(meta("claude", "still-current", 1))
            .expect("initial row");
        let current = initial.publish().expect("publish initial");
        let pointer_path = store.scope_dir("claude").join(POINTER_FILE);
        let pointer_before = fs::read(&pointer_path).expect("read current pointer");

        let mut broken = store.begin_build("claude").expect("broken build");
        for index in 0..=PAGE_SIZE {
            broken
                .push(meta(
                    "claude",
                    &format!("must-not-publish-{index:03}"),
                    index as i64 + 2,
                ))
                .expect("broken row");
        }
        let broken_generation = broken.generation.clone();
        let broken_staging = broken.staging_dir.clone();
        broken.corrupt_page_before_validation = Some(1);
        assert!(broken.publish().is_err());

        assert_eq!(
            fs::read(&pointer_path).expect("read retained pointer"),
            pointer_before
        );
        assert!(!store.generation_dir("claude", &broken_generation).exists());
        assert!(!broken_staging.exists());
        let page = store.load_page("claude", 0).expect("retained page");
        assert_eq!(page.generation, current.generation);
        assert_eq!(page.rows[0].session_id, "still-current");
    }

    #[test]
    fn staging_validation_does_not_block_an_unrelated_scope_begin_build() {
        let (_temp, store) = test_store();
        let mut builder = store.begin_build("claude").expect("begin validation build");
        for index in 0..=PAGE_SIZE {
            builder
                .push(meta("claude", &format!("row-{index:03}"), index as i64))
                .expect("push row");
        }
        let validation_started = Arc::new(std::sync::Barrier::new(2));
        let validation_resume = Arc::new(std::sync::Barrier::new(2));
        builder.validation_barriers = Some((
            Arc::clone(&validation_started),
            Arc::clone(&validation_resume),
        ));

        let publisher = std::thread::spawn(move || builder.publish());
        validation_started.wait();

        let other_scope_store = store.clone();
        let (began_tx, began_rx) = std::sync::mpsc::sync_channel(1);
        let other_scope = std::thread::spawn(move || {
            let result = other_scope_store.begin_build("codex");
            let _ = began_tx.send(result.is_ok());
            drop(result);
        });
        let began_while_validation_was_paused = began_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or(false);

        validation_resume.wait();
        publisher
            .join()
            .expect("publisher thread")
            .expect("publish validated generation");
        other_scope.join().expect("unrelated begin thread");
        assert!(
            began_while_validation_was_paused,
            "staging validation held the process-global coordinator mutex"
        );
    }

    #[test]
    fn newer_same_scope_build_wins_while_old_build_validates_staging() {
        let (_temp, store) = test_store();
        let mut old = store.begin_build("claude").expect("begin old build");
        old.push(meta("claude", "old", 1)).expect("push old row");
        let validation_started = Arc::new(std::sync::Barrier::new(2));
        let validation_resume = Arc::new(std::sync::Barrier::new(2));
        old.validation_barriers = Some((
            Arc::clone(&validation_started),
            Arc::clone(&validation_resume),
        ));

        let old_publisher = std::thread::spawn(move || old.publish());
        validation_started.wait();

        let mut newer = store.begin_build("claude").expect("begin newer build");
        newer
            .push(meta("claude", "newer", 2))
            .expect("push newer row");
        newer.publish().expect("publish newer build");

        validation_resume.wait();
        assert!(matches!(
            old_publisher.join().expect("old publisher thread"),
            Err(ManifestError::Cancelled)
        ));
        assert_eq!(
            store.load_page("claude", 0).expect("current page").rows[0].session_id,
            "newer"
        );
    }

    #[test]
    fn external_multi_pass_merge_is_stable_and_bounded_by_options() {
        let (_temp, store) = test_store();
        let mut builder = store
            .begin_build_with_options(
                "all",
                BuildOptions {
                    spill_rows: 7,
                    merge_fan_in: 3,
                },
            )
            .expect("begin");
        for index in (0..350).rev() {
            let provider = if index % 2 == 0 { "claude" } else { "codex" };
            builder
                .push(meta(provider, &format!("s-{index:03}"), index % 11))
                .expect("push");
            assert!(builder.buffer.len() < 7);
        }
        assert!(
            builder.peak_retained_run_paths() <= 16,
            "layered merge retained too many run paths: {}",
            builder.peak_retained_run_paths()
        );
        builder.publish().expect("publish");

        let reader = store.open_reader("all").expect("reader");
        let mut rows = Vec::new();
        for page in 0..reader.page_count() {
            rows.extend(reader.load_page(page).expect("page").rows);
        }
        assert_eq!(rows.len(), 350);
        assert!(rows
            .windows(2)
            .all(|pair| compare_rows(&pair[0], &pair[1]) != Ordering::Greater));
    }

    #[test]
    fn publish_deduplicates_identity_across_spills_and_page_boundaries() {
        let (_temp, store) = test_store();
        let mut builder = store
            .begin_build_with_options(
                "claude",
                BuildOptions {
                    spill_rows: 3,
                    merge_fan_in: 2,
                },
            )
            .expect("begin");
        for index in 0..205 {
            builder
                .push(meta("claude", &format!("s-{index:03}"), index))
                .expect("push");
        }
        builder
            .push(meta("claude", "s-050", 10_000))
            .expect("newer duplicate");
        let published = builder.publish().expect("publish");

        assert_eq!(published.total_rows, 205);
        let reader = store.open_reader("claude").expect("reader");
        let mut duplicates = Vec::new();
        for page_index in 0..reader.page_count() {
            duplicates.extend(
                reader
                    .load_page(page_index)
                    .expect("page")
                    .rows
                    .into_iter()
                    .filter(|row| row.session_id == "s-050"),
            );
        }
        assert_eq!(duplicates.len(), 1);
        assert_eq!(duplicates[0].last_active_at, Some(10_000));
    }

    #[test]
    fn identity_row_enrichment_reaches_pages_without_breaking_deduplication() {
        let (_temp, store) = test_store();
        let visits = Arc::new(Mutex::new(Vec::new()));
        let mut builder = store
            .begin_build_with_options(
                "codex",
                BuildOptions {
                    spill_rows: 1,
                    merge_fan_in: 2,
                },
            )
            .expect("begin");
        builder
            .add_identity_enricher(Box::new({
                let visits = Arc::clone(&visits);
                move |row: &mut SessionMeta| {
                    let recency = row.last_active_at.unwrap_or_default();
                    visits
                        .lock()
                        .expect("record enrichment visit")
                        .push(recency);
                    row.title = Some(format!("enriched {recency}"));
                }
            }))
            .expect("add identity enricher");

        builder
            .push(meta("codex", "duplicate", 1))
            .expect("push older duplicate");
        builder
            .push(meta("codex", "duplicate", 10))
            .expect("push newer duplicate");
        let published = builder.publish().expect("publish");

        assert_eq!(published.total_rows, 1);
        assert_eq!(published.first_page.rows.len(), 1);
        assert_eq!(
            published.first_page.rows[0].title.as_deref(),
            Some("enriched 10")
        );
        assert_eq!(
            published.first_page.rows[0].last_active_at,
            Some(10),
            "the newest duplicate must remain authoritative"
        );
        assert_eq!(
            *visits.lock().expect("read enrichment visits"),
            vec![10, 1],
            "identity-run rows must be enriched in their sorted order before dedupe"
        );
    }

    #[test]
    fn newer_build_cancels_old_build_and_old_cannot_publish() {
        let (_temp, store) = test_store();
        let mut old = store.begin_build("claude").expect("old");
        old.push(meta("claude", "old", 1)).expect("push old");
        let mut new = store.begin_build("claude").expect("new");
        assert!(old.is_cancelled());
        new.push(meta("claude", "new", 2)).expect("push new");
        new.publish().expect("publish new");

        assert!(matches!(old.publish(), Err(ManifestError::Cancelled)));
        let page = store.load_page("claude", 0).expect("current page");
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].session_id, "new");
    }

    #[test]
    fn persisted_epoch_rejects_old_builder_without_process_local_coordinator() {
        let (temp, store) = test_store();
        let mut old = store.begin_build("claude").expect("old");
        old.push(meta("claude", "old", 1)).expect("push old");

        // Simulate another process: it shares disk state and file locks, but not
        // this process's Weak<AtomicBool> cancellation registry.
        forget_process_local_scope(&store, "claude");
        let other = PagedManifestStore::open_at(temp.path()).expect("other store");
        let mut new = other.begin_build("claude").expect("new");
        new.push(meta("claude", "new", 2)).expect("push new");
        new.publish().expect("publish new");

        assert!(!old.is_cancelled());
        assert!(matches!(old.publish(), Err(ManifestError::Cancelled)));
        let page = other.load_page("claude", 0).expect("current page");
        assert_eq!(page.rows[0].session_id, "new");
    }

    #[test]
    fn truncated_coordinator_fails_closed_and_a_fresh_build_recovers() {
        let (_temp, store) = test_store();
        let mut initial = store.begin_build("claude").expect("initial build");
        initial.push(meta("claude", "old", 1)).expect("old row");
        let old = initial.publish().expect("publish old");
        let old_reader = old.reader.clone();
        let old_generation = old.generation.clone();
        let old_domain = old_reader.header.epoch_domain.clone();

        fs::write(store.coordinator_path("claude"), b"{\"formatVersion\":")
            .expect("truncate coordinator");
        assert!(store.open_reader("claude").is_none());
        assert!(old_reader.load_page(0).is_none());

        let mut replacement = store.begin_build("claude").expect("recover build");
        assert!(store.open_reader("claude").is_none());
        assert!(store.open_generation("claude", &old_generation).is_none());
        replacement
            .push(meta("claude", "fresh", 2))
            .expect("fresh row");
        let fresh = replacement.publish().expect("publish recovered generation");

        assert_ne!(fresh.reader.header.epoch_domain, old_domain);
        assert!(fresh.build_epoch > old.build_epoch);
        assert_eq!(fresh.first_page.rows[0].session_id, "fresh");
        assert_eq!(
            store.load_page("claude", 0).expect("recovered page").rows[0].session_id,
            "fresh"
        );
    }

    #[test]
    fn truncated_read_floor_fails_closed_and_a_fresh_build_recovers() {
        let (_temp, store) = test_store();
        let mut initial = store.begin_build("claude").expect("initial build");
        initial.push(meta("claude", "old", 1)).expect("old row");
        let old = initial.publish().expect("publish old");
        let old_reader = old.reader.clone();
        let old_domain = old_reader.header.epoch_domain.clone();

        fs::write(store.read_floor_path("claude"), b"{\"formatVersion\":")
            .expect("truncate read floor");
        assert!(store.open_reader("claude").is_none());
        assert!(old_reader.load_page(0).is_none());

        let mut replacement = store.begin_build("claude").expect("recover build");
        replacement
            .push(meta("claude", "fresh", 2))
            .expect("fresh row");
        let fresh = replacement.publish().expect("publish recovered generation");

        assert_ne!(fresh.reader.header.epoch_domain, old_domain);
        assert!(fresh.build_epoch > old.build_epoch);
        assert_eq!(fresh.first_page.rows[0].session_id, "fresh");
        assert_eq!(
            store.load_page("claude", 0).expect("recovered page").rows[0].session_id,
            "fresh"
        );
    }

    #[test]
    fn delete_cancels_late_build_and_cannot_be_resurrected() {
        let (_temp, store) = test_store();
        let gone = meta("claude", "gone", 20);
        let kept = meta("claude", "kept", 10);
        let mut initial = store.begin_build("claude").expect("initial");
        initial.push(gone.clone()).expect("gone");
        initial.push(kept.clone()).expect("kept");
        initial.publish().expect("publish initial");

        let mut late = store.begin_build("claude").expect("late");
        late.push(gone.clone()).expect("stale gone");
        late.push(meta("claude", "new", 30)).expect("new");

        store
            .purge_identity(
                "claude",
                "claude",
                "gone",
                gone.source_path.as_deref().expect("source"),
            )
            .expect("purge")
            .expect("published purge");
        assert!(matches!(late.publish(), Err(ManifestError::Cancelled)));

        let page = store.load_page("claude", 0).expect("page");
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].session_id, "kept");
    }

    #[test]
    fn persisted_delete_epoch_rejects_a_late_builder_from_another_coordinator() {
        let (temp, store) = test_store();
        let gone = meta("claude", "gone", 20);
        let kept = meta("claude", "kept", 10);
        let mut initial = store.begin_build("claude").expect("initial");
        initial.push(gone.clone()).expect("gone");
        initial.push(kept).expect("kept");
        initial.publish().expect("publish initial");

        let mut stale = store.begin_build("claude").expect("stale");
        stale.push(gone.clone()).expect("stale gone");
        forget_process_local_scope(&store, "claude");
        let other = PagedManifestStore::open_at(temp.path()).expect("other store");
        other
            .purge_identity(
                "claude",
                "claude",
                "gone",
                gone.source_path.as_deref().expect("source"),
            )
            .expect("purge")
            .expect("published purge");

        assert!(!stale.is_cancelled());
        assert!(matches!(stale.publish(), Err(ManifestError::Cancelled)));
        let page = other.load_page("claude", 0).expect("page");
        assert!(page.rows.iter().all(|row| row.session_id != "gone"));
    }

    #[test]
    fn refresh_started_after_repack_source_capture_prevents_stale_publish() {
        let (temp, store) = test_store();
        let mut initial = store.begin_build("claude").expect("initial");
        initial
            .push(meta("claude", "source", 1))
            .expect("source row");
        initial.publish().expect("publish source");

        // Model the purge/repack ordering: win an epoch first, then pin the
        // exact current source generation. A refresh from another process that
        // starts afterward must win without allowing this stale repack to
        // overwrite it.
        let mut repack = store.begin_build("claude").expect("repack");
        let source = store.open_reader("claude").expect("repack source");
        repack.bind_source(&source).expect("bind source");
        repack
            .push(meta("claude", "repacked", 2))
            .expect("repacked row");

        forget_process_local_scope(&store, "claude");
        let other = PagedManifestStore::open_at(temp.path()).expect("other process store");
        let mut refresh = other.begin_build("claude").expect("newer refresh");
        refresh
            .push(meta("claude", "refreshed", 3))
            .expect("refreshed row");
        refresh.publish().expect("publish newer refresh");

        assert!(
            !repack.is_cancelled(),
            "test must exercise persisted epoch/source validation"
        );
        assert!(matches!(repack.publish(), Err(ManifestError::Cancelled)));
        let page = other.load_page("claude", 0).expect("current page");
        assert_eq!(page.rows[0].session_id, "refreshed");
    }

    #[test]
    fn tombstone_without_existing_manifest_filters_next_build() {
        let (_temp, store) = test_store();
        let gone = meta("claude", "gone", 20);
        assert!(store
            .purge_identity(
                "claude",
                "claude",
                "gone",
                gone.source_path.as_deref().expect("source"),
            )
            .expect("purge")
            .is_none());

        let mut builder = store.begin_build("claude").expect("build");
        builder.push(gone).expect("stale row");
        builder.push(meta("claude", "kept", 10)).expect("kept");
        builder.publish().expect("publish");
        let page = store.load_page("claude", 0).expect("page");
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].session_id, "kept");
    }

    #[test]
    fn tombstones_deduplicate_and_overflow_invalidates_old_generations_safely() {
        let (_temp, store) = test_store();
        let mut initial = store.begin_build("claude").expect("initial");
        initial.push(meta("claude", "old", 1)).expect("old row");
        let published = initial.publish().expect("publish initial");

        {
            let _scope_lock =
                FileLock::exclusive(&store.scope_lock_path("claude")).expect("scope lock");
            let mut disk = store
                .read_disk_coordinator_locked("claude")
                .expect("read coordinator");
            disk.build_epoch = next_epoch(disk.build_epoch).expect("epoch");
            store
                .record_tombstone_bounded(
                    "claude",
                    &mut disk,
                    Tombstone {
                        generation: 1,
                        provider_id: "claude".to_string(),
                        session_id: "same".to_string(),
                        source_path: "/same".to_string(),
                    },
                )
                .expect("first tombstone");
            store
                .record_tombstone_bounded(
                    "claude",
                    &mut disk,
                    Tombstone {
                        generation: 2,
                        provider_id: "claude".to_string(),
                        session_id: "same".to_string(),
                        source_path: "/same".to_string(),
                    },
                )
                .expect("deduplicated tombstone");
            assert_eq!(disk.tombstones.len(), 1);
            assert_eq!(disk.tombstones[0].generation, 2);

            disk.tombstones = (0..MAX_TOMBSTONES)
                .map(|index| Tombstone {
                    generation: index as u64,
                    provider_id: "claude".to_string(),
                    session_id: format!("deleted-{index}"),
                    source_path: format!("/deleted/{index}"),
                })
                .collect();
            store
                .record_tombstone_bounded(
                    "claude",
                    &mut disk,
                    Tombstone {
                        generation: MAX_TOMBSTONES as u64,
                        provider_id: "claude".to_string(),
                        session_id: "overflow".to_string(),
                        source_path: "/overflow".to_string(),
                    },
                )
                .expect("bounded overflow");
            assert!(disk.tombstones.is_empty());
            assert_eq!(disk.min_readable_epoch, disk.build_epoch);
            store
                .write_disk_coordinator_locked("claude", &disk)
                .expect("persist invalidation");
        }

        assert!(store.open_reader("claude").is_none());
        assert!(store
            .open_generation("claude", &published.generation)
            .is_none());

        let mut replacement = store.begin_build("claude").expect("replacement");
        replacement
            .push(meta("claude", "fresh", 2))
            .expect("fresh row");
        replacement.publish().expect("publish replacement");
        assert_eq!(
            store.load_page("claude", 0).expect("fresh page").rows[0].session_id,
            "fresh"
        );
    }

    #[test]
    fn leased_reader_survives_multiple_new_publications() {
        let (_temp, store) = test_store();
        let mut first = store.begin_build("claude").expect("first");
        for index in 0..101 {
            first
                .push(meta("claude", &format!("old-{index}"), index))
                .expect("push");
        }
        let first_publication = first.publish().expect("publish first");
        // Use the lease returned atomically by publication. No later
        // `open_generation` is allowed to bridge a publish/GC race.
        let old_reader = first_publication.reader.clone();
        let old_generation = old_reader.generation().to_string();
        forget_process_local_scope(&store, "claude");

        for round in 0..3 {
            let mut next = store.begin_build("claude").expect("next");
            next.push(meta("claude", &format!("new-{round}"), 1_000 + round))
                .expect("push");
            next.publish().expect("publish next");
        }

        assert_ne!(
            store.open_reader("claude").expect("current").generation(),
            old_generation
        );
        let old_second = old_reader.load_page(1).expect("leased old page");
        assert_eq!(old_second.rows.len(), 1);
        assert_eq!(old_second.generation, old_generation);
    }

    #[test]
    fn overlapping_cli_and_tui_builds_have_independent_cancellation_domains() {
        let temp = tempdir().expect("tempdir");
        let authoritative = PagedManifestStore::open_at(temp.path()).expect("TUI store");
        let mut tui_build = authoritative.begin_build("claude").expect("TUI build");
        tui_build.push(meta("claude", "tui", 30)).expect("TUI row");

        let cli_a = CliManifestStore::open_at(temp.path()).expect("CLI A store");
        let mut cli_a_build = cli_a.begin_build("claude").expect("CLI A build");
        cli_a_build
            .push(meta("claude", "cli-a", 20))
            .expect("CLI A row");

        // Opening B runs stale-namespace cleanup while A is still building.
        // A's shared namespace lease must keep both its epoch and files alive.
        let cli_b = CliManifestStore::open_at(temp.path()).expect("CLI B store");
        let mut cli_b_build = cli_b.begin_build("claude").expect("CLI B build");
        cli_b_build
            .push(meta("claude", "cli-b", 10))
            .expect("CLI B row");

        assert!(!tui_build.is_cancelled());
        assert!(!cli_a_build.is_cancelled());
        assert!(!cli_b_build.is_cancelled());
        assert_ne!(authoritative.root, cli_a.inner.root);
        assert_ne!(cli_a.inner.root, cli_b.inner.root);

        let tui = tui_build.publish().expect("publish TUI");
        let a = cli_a_build.publish().expect("publish CLI A");
        let b = cli_b_build.publish().expect("publish CLI B");

        assert_eq!(tui.build_epoch, 1);
        assert_eq!(a.build_epoch, 1);
        assert_eq!(b.build_epoch, 1);
        assert_eq!(tui.first_page.rows[0].session_id, "tui");
        assert_eq!(a.first_page.rows[0].session_id, "cli-a");
        assert_eq!(b.first_page.rows[0].session_id, "cli-b");
        assert_eq!(
            authoritative
                .open_reader("claude")
                .expect("authoritative reader")
                .load_page(0)
                .expect("authoritative page")
                .rows[0]
                .session_id,
            "tui"
        );

        // Isolation changes only the physical domain. Within one CLI
        // namespace, the newer epoch still wins and the old build cannot
        // publish over it.
        let mut stale = cli_a.begin_build("claude").expect("stale CLI build");
        stale.push(meta("claude", "stale", 40)).expect("stale row");
        let mut replacement = cli_a.begin_build("claude").expect("replacement CLI build");
        replacement
            .push(meta("claude", "replacement", 50))
            .expect("replacement row");
        assert!(stale.is_cancelled());
        let replacement = replacement.publish().expect("publish replacement");
        assert_eq!(replacement.build_epoch, 3);
        assert!(matches!(stale.publish(), Err(ManifestError::Cancelled)));
    }

    #[test]
    fn cli_namespace_is_removed_only_after_the_last_reader_clone_drops() {
        let temp = tempdir().expect("tempdir");
        let store = CliManifestStore::open_at(temp.path()).expect("CLI store");
        let namespace_root = store.inner.root.clone();
        let mut builder = store.begin_build("claude").expect("CLI build");
        builder.push(meta("claude", "kept", 1)).expect("CLI row");
        let published = builder.publish().expect("publish CLI snapshot");
        let reader = published.reader.clone();
        let last_reader = reader.clone();

        drop(published);
        drop(store);
        assert!(namespace_root.is_dir(), "reader lease must retain pages");
        assert_eq!(
            reader.load_page(0).expect("page retained by reader").rows[0].session_id,
            "kept"
        );

        drop(reader);
        assert!(
            namespace_root.is_dir(),
            "one remaining reader clone must retain the namespace"
        );
        drop(last_reader);
        assert!(
            !namespace_root.exists(),
            "the last clone must synchronously remove its one-shot namespace"
        );
    }

    #[test]
    fn large_cli_namespace_deletion_does_not_hold_the_root_lock() {
        let temp = tempdir().expect("tempdir");
        let mut store = CliManifestStore::open_at(temp.path()).expect("CLI store");
        let namespace_root = store.inner.root.clone();
        for index in 0..4_096 {
            drop(
                create_private_file(&namespace_root.join(format!("large-{index:04}")))
                    .expect("large namespace entry"),
            );
        }

        let deletion_started = Arc::new(std::sync::Barrier::new(2));
        let resume_deletion = Arc::new(std::sync::Barrier::new(2));
        Arc::get_mut(
            store
                .inner
                ._namespace_cleanup
                .as_mut()
                .expect("CLI cleanup guard"),
        )
        .expect("unshared cleanup guard")
        .deletion_barriers = Some((Arc::clone(&deletion_started), Arc::clone(&resume_deletion)));

        drop(store);
        deletion_started.wait();
        assert!(
            !namespace_root.exists(),
            "the readable namespace must be renamed before recursive deletion"
        );

        let config_dir = temp.path().to_path_buf();
        let (opened_tx, opened_rx) = std::sync::mpsc::channel();
        let opener = std::thread::spawn(move || {
            let result = CliManifestStore::open_at(&config_dir);
            let _ = opened_tx.send(result.is_ok());
        });
        assert_eq!(
            opened_rx.recv_timeout(std::time::Duration::from_secs(2)),
            Ok(true),
            "another CLI open must not wait for recursive deletion"
        );
        resume_deletion.wait();
        opener.join().expect("CLI opener");
    }

    #[test]
    fn cli_namespace_cleanup_covers_cancelled_and_crashed_builds() {
        let temp = tempdir().expect("tempdir");
        let store = CliManifestStore::open_at(temp.path()).expect("CLI store");
        let cancelled_root = store.inner.root.clone();
        let stale = store.begin_build("claude").expect("stale build");
        let replacement = store.begin_build("claude").expect("replacement build");
        assert!(stale.is_cancelled());
        drop(stale);
        drop(replacement);
        drop(store);
        assert!(
            !cancelled_root.exists(),
            "cancelled builders must not leave a namespace"
        );

        // Model a process that exited without running Drop. The next opener
        // removes its unleased namespace before starting another CLI build.
        let cli_root = temp.path().join(CLI_ROOT_DIR);
        create_private_dir(&cli_root).expect("CLI root");
        let crashed_root = cli_root.join("cli-00000000000000000000000000000000");
        create_private_dir(&crashed_root).expect("crashed namespace");
        drop(
            open_lock_file(&crashed_root.join(NAMESPACE_LOCK_FILE))
                .expect("crashed namespace lock"),
        );

        let recovered = CliManifestStore::open_at(temp.path()).expect("recovered CLI store");
        let recovered_root = recovered.inner.root.clone();
        assert!(
            !crashed_root.exists(),
            "startup must synchronously remove an unleased crash artifact"
        );
        drop(recovered);
        assert!(!recovered_root.exists());
    }

    #[test]
    fn query_namespaces_have_independent_epochs_and_current_pointers() {
        let temp = tempdir().expect("tempdir");
        let base_store = PagedManifestStore::open_at(temp.path()).expect("base store");
        let mut base_builder = base_store.begin_build("claude").expect("base build");
        base_builder
            .push(meta("claude", "base", 1))
            .expect("base row");
        let base = base_builder.publish().expect("publish base");

        let namespace_a = QueryManifestNamespace::for_test("tui-query-a");
        let namespace_b = QueryManifestNamespace::for_test("tui-query-b");
        let query_a = QueryManifestStore::open_at(temp.path(), &namespace_a).expect("query A");
        let query_b = QueryManifestStore::open_at(temp.path(), &namespace_b).expect("query B");

        let mut pending_b = query_b.begin_build(&base.reader).expect("begin B");
        pending_b
            .push(meta("claude", "result-b", 20))
            .expect("B row");
        let mut build_a = query_a.begin_build(&base.reader).expect("begin A");
        build_a.push(meta("claude", "result-a", 10)).expect("A row");
        let published_a = build_a.publish().expect("publish A");

        assert!(
            !pending_b.is_cancelled(),
            "one TUI query must not cancel another namespace"
        );
        let published_b = pending_b.publish().expect("publish B");
        assert_eq!(published_a.build_epoch, 1);
        assert_eq!(published_b.build_epoch, 1);
        assert_ne!(published_a.generation, published_b.generation);
        assert_eq!(
            query_a
                .open_reader("claude")
                .expect("A reader")
                .load_page(0)
                .expect("A page")
                .rows[0]
                .session_id,
            "result-a"
        );
        assert_eq!(
            query_b
                .open_reader("claude")
                .expect("B reader")
                .load_page(0)
                .expect("B page")
                .rows[0]
                .session_id,
            "result-b"
        );
    }

    #[test]
    fn query_namespace_is_retired_after_its_last_reader_drops() {
        let temp = tempdir().expect("tempdir");
        let base_store = PagedManifestStore::open_at(temp.path()).expect("base store");
        let mut base_builder = base_store.begin_build("claude").expect("base build");
        base_builder
            .push(meta("claude", "base", 1))
            .expect("base row");
        let base = base_builder.publish().expect("publish base");

        let namespace = QueryManifestNamespace::for_test("tui-query-drop");
        let query = QueryManifestStore::open_at(temp.path(), &namespace).expect("query store");
        let namespace_root = query.inner.root.clone();
        let mut builder = query.begin_build(&base.reader).expect("query build");
        builder.push(meta("claude", "match", 2)).expect("query row");
        let published = builder.publish().expect("publish query");
        let reader = published.reader.clone();

        drop(published);
        drop(query);
        assert!(namespace_root.is_dir());
        assert_eq!(
            reader.load_page(0).expect("leased query page").rows[0].session_id,
            "match"
        );
        drop(reader);
        assert!(
            !namespace_root.exists(),
            "the last query reader must retire the namespace immediately"
        );
    }

    #[test]
    fn open_is_fixed_cost_and_begin_build_cleans_only_unowned_artifacts() {
        let (temp, store) = test_store();
        let scope_dir = store.scope_dir("claude");
        create_private_dir(&scope_dir).expect("scope");
        let stale_generation = "gen-00000000000000000000000000000001-00000001-0000000000000001";
        let stale_build = scope_dir.join(format!(".{stale_generation}.building"));
        create_private_dir(&stale_build).expect("stale build");
        drop(open_lock_file(&stale_build.join(BUILD_OWNER_FILE)).expect("owner"));
        let stale_gc = scope_dir.join(format!(".{stale_generation}.gc"));
        create_private_dir(&stale_gc).expect("stale gc");

        let _reopened = PagedManifestStore::open_at(temp.path()).expect("fixed-cost reopen");
        assert!(
            stale_build.exists(),
            "open must not walk or delete artifacts"
        );
        assert!(
            stale_gc.exists(),
            "open must not recursively delete artifacts"
        );

        let active = store.begin_build("claude").expect("active build");
        let active_dir = active.staging_dir.clone();
        assert!(active_dir.is_dir());
        assert!(!stale_build.exists());
        assert!(!stale_gc.exists());

        let replacement = store.begin_build("claude").expect("replacement build");
        assert!(
            active_dir.is_dir(),
            "active owner lock must protect staging"
        );
        drop(replacement);
        drop(active);
        assert!(!active_dir.exists());
    }

    #[test]
    fn explicit_generation_open_is_not_redirected_to_current() {
        let (_temp, store) = test_store();
        let mut first = store.begin_build("claude").expect("first");
        first.push(meta("claude", "old", 1)).expect("old");
        let old_generation = first.publish().expect("publish old").generation;

        let mut second = store.begin_build("claude").expect("second");
        second.push(meta("claude", "new", 2)).expect("new");
        let new_generation = second.publish().expect("publish new").generation;
        assert_ne!(old_generation, new_generation);

        let old = store
            .open_generation("claude", &old_generation)
            .expect("old generation");
        assert_eq!(
            old.load_page(0).expect("old page").rows[0].session_id,
            "old"
        );
        assert_eq!(
            store.load_page("claude", 0).expect("current").rows[0].session_id,
            "new"
        );
    }

    #[test]
    fn corrupt_next_page_does_not_double_read_it_from_current_page() {
        let (_temp, store) = test_store();
        let mut builder = store.begin_build("claude").expect("begin");
        for index in 0..101 {
            builder
                .push(meta("claude", &format!("s-{index}"), index))
                .expect("push");
        }
        let published = builder.publish().expect("publish");
        let next_path = store
            .generation_dir("claude", &published.generation)
            .join(page_file_name(1));
        fs::write(&next_path, b"not-json").expect("corrupt next page");

        let first = store.load_page("claude", 0).expect("first page");
        assert!(first.has_next);
        assert!(store.load_page("claude", 1).is_none());
    }

    #[test]
    fn bounded_preview_keeps_only_true_top_k() {
        let mut preview = BoundedRecencyPreview::new(3);
        for recency in [3, 100, 1, 7, 50, 2] {
            preview.push(meta("claude", &format!("s-{recency}"), recency));
        }
        let rows = preview.into_sorted();
        let recencies: Vec<_> = rows.into_iter().map(|row| row.last_active_at).collect();
        assert_eq!(recencies, vec![Some(100), Some(50), Some(7)]);
    }

    #[test]
    fn rejects_scope_mismatch_and_path_like_scope() {
        let (_temp, store) = test_store();
        assert!(matches!(
            store.begin_build("../claude"),
            Err(ManifestError::UnsupportedScope(_))
        ));
        let mut builder = store.begin_build("claude").expect("begin");
        assert!(matches!(
            builder.push(meta("codex", "wrong", 1)),
            Err(ManifestError::RowOutsideScope { .. })
        ));
    }
}
