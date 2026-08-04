//! Provider-neutral, disk-indexed transcript paging.
//!
//! Provider session files and databases remain the source of truth. This module
//! writes only disposable message locators into a private sidecar SQLite file,
//! then materializes one bounded page from the provider source on demand.
//! Immutable generations and shared reader leases keep page boundaries stable
//! while a refreshed index is published.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rusqlite::{params, Connection, OptionalExtension};
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::providers::{claude, codex, gemini, hermes, openclaw, opencode};
use super::{truncate_string_utf8, SessionMessage};
use crate::config::{
    create_managed_config_dir_all, get_app_config_dir,
    resolve_config_dir_without_following_user_symlinks, write_json_file,
};

pub(crate) const TRANSCRIPT_PAGE_SIZE: usize = 100;
const FORMAT_VERSION: u32 = 3;
const ROOT_DIR: &str = "transcript-index-v1";
const CURRENT_FILE: &str = "current.json";
const MANIFEST_FILE: &str = "manifest.json";
const INDEX_FILE: &str = "index.sqlite";
const ROOT_LOCK_FILE: &str = ".root.lock";
const CACHE_BUILD_LOCK_FILE: &str = ".cache-build.lock";
const SCOPE_LOCK_FILE: &str = ".scope.lock";
const SCOPE_LEASE_FILE: &str = ".scope.lease";
const BUILD_LOCK_FILE: &str = ".build.lock";
const GENERATION_LEASE_FILE: &str = ".lease.lock";
const INVALID_GENERATION_FILE: &str = ".invalid";
const PURGE_SCOPE_FILE: &str = ".purge";
const MAX_POINTER_BYTES: u64 = 64 * 1024;
const MAX_MANIFEST_BYTES: u64 = 128 * 1024;
const MAX_INDEXABLE_RECORD_BYTES: usize = 4 * 1024 * 1024;
const MAX_SOURCE_STABILITY_ATTEMPTS: usize = 3;
const MAX_CACHED_TRANSCRIPT_SCOPES: usize = 128;
const MAX_TRANSCRIPT_CACHE_BYTES: u64 = 512 * 1024 * 1024;
const INITIAL_TRANSCRIPT_BUILD_BYTES: u64 = 64 * 1024 * 1024;
const MIN_TRANSCRIPT_BUILD_BYTES: u64 = 1024 * 1024;
const TRANSCRIPT_BUILD_METADATA_RESERVE: u64 = 1024 * 1024;
const BUILD_BUDGET_EXCEEDED_ERROR: &str = "__cc_switch_transcript_build_budget_exceeded__";

const LOCATOR_JSONL: i64 = 1;
const LOCATOR_RAW_JSONL: i64 = 2;
const LOCATOR_GEMINI: i64 = 3;
const LOCATOR_RAW_GEMINI: i64 = 4;
const LOCATOR_OPENCODE_FILE: i64 = 5;
const LOCATOR_OPENCODE_SQLITE: i64 = 6;
const LOCATOR_HERMES_SQLITE: i64 = 7;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TranscriptPage {
    pub(crate) generation: String,
    pub(crate) page_index: usize,
    pub(crate) total_rows: usize,
    pub(crate) messages: Vec<SessionMessage>,
    pub(crate) message_keys: Vec<String>,
    pub(crate) has_previous: bool,
    pub(crate) has_next: bool,
    /// Every logical message remains reachable. This flag only means one or
    /// more message bodies on this page were shortened for bounded display.
    pub(crate) content_truncated: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct TranscriptReader {
    manifest: TranscriptManifest,
    db_path: PathBuf,
    generation_dir: PathBuf,
    _lease: Arc<FileLock>,
    _scope_lease: Arc<ScopeLease>,
    source_checkpoint: Arc<Mutex<SourceCheckpoint>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptPageError {
    RefreshRequired(String),
    Other(String),
}

impl TranscriptPageError {
    pub(crate) const fn requires_refresh(&self) -> bool {
        matches!(self, Self::RefreshRequired(_))
    }
}

impl std::fmt::Display for TranscriptPageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RefreshRequired(message) | Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for TranscriptPageError {}

impl TranscriptReader {
    pub(crate) fn generation(&self) -> &str {
        &self.manifest.generation
    }

    pub(crate) const fn total_rows(&self) -> usize {
        self.manifest.total_rows
    }

    pub(crate) fn provider_id(&self) -> &str {
        &self.manifest.provider_id
    }

    pub(crate) fn source_path(&self) -> &str {
        &self.manifest.source_path
    }

    pub(crate) fn page_count(&self) -> usize {
        self.manifest.total_rows.div_ceil(TRANSCRIPT_PAGE_SIZE)
    }

    pub(crate) fn load_page(
        &self,
        page_index: usize,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<TranscriptPage, TranscriptPageError> {
        if is_cancelled() {
            return Err(TranscriptPageError::Other(
                "Session message page was cancelled".to_string(),
            ));
        }
        self.ensure_source_is_current(is_cancelled)?;
        if self.manifest.total_rows == 0 {
            if page_index != 0 {
                return Err(TranscriptPageError::Other(
                    "Session message page is outside the transcript".to_string(),
                ));
            }
            self.ensure_source_is_current(is_cancelled)?;
            return Ok(TranscriptPage {
                generation: self.manifest.generation.clone(),
                page_index,
                total_rows: 0,
                messages: Vec::new(),
                message_keys: Vec::new(),
                has_previous: false,
                has_next: false,
                content_truncated: false,
            });
        }
        if page_index >= self.page_count() {
            return Err(TranscriptPageError::Other(
                "Session message page is outside the transcript".to_string(),
            ));
        }

        let start = page_index
            .checked_mul(TRANSCRIPT_PAGE_SIZE)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| {
                TranscriptPageError::Other("Session message page offset overflowed".to_string())
            })?;
        let end = start
            .saturating_add(TRANSCRIPT_PAGE_SIZE)
            .saturating_sub(1)
            .min(self.manifest.total_rows);
        let conn = Connection::open_with_flags(
            &self.db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|error| {
            self.mark_generation_invalid();
            TranscriptPageError::RefreshRequired(format!(
                "Failed to open transcript index: {error}"
            ))
        })?;
        let mut stmt = conn
            .prepare(
                "SELECT locator_kind, locator_text, locator_offset, locator_length,
                        message_key
                 FROM entries
                 WHERE ordinal BETWEEN ?1 AND ?2
                 ORDER BY ordinal ASC",
            )
            .map_err(|error| {
                self.mark_generation_invalid();
                TranscriptPageError::RefreshRequired(format!(
                    "Failed to prepare transcript page query: {error}"
                ))
            })?;
        let expected_count = end.saturating_sub(start).saturating_add(1);
        let db_start = usize_to_i64(start).map_err(TranscriptPageError::Other)?;
        let db_end = usize_to_i64(end).map_err(TranscriptPageError::Other)?;
        let rows = stmt
            .query_map(params![db_start, db_end], |row| {
                Ok(Locator {
                    kind: row.get(0)?,
                    text: row.get(1)?,
                    offset: row.get(2)?,
                    length: row.get(3)?,
                    message_key: row.get(4)?,
                })
            })
            .map_err(|error| {
                self.mark_generation_invalid();
                TranscriptPageError::RefreshRequired(format!(
                    "Failed to query transcript page: {error}"
                ))
            })?;
        let mut locators = Vec::with_capacity(expected_count);
        for row in rows {
            if is_cancelled() {
                return Err(TranscriptPageError::Other(
                    "Session message page was cancelled".to_string(),
                ));
            }
            locators.push(row.map_err(|error| {
                self.mark_generation_invalid();
                TranscriptPageError::RefreshRequired(format!(
                    "Failed to decode transcript locator: {error}"
                ))
            })?);
        }
        if locators.len() != expected_count {
            self.mark_generation_invalid();
            return Err(TranscriptPageError::RefreshRequired(
                "Transcript index page is incomplete".to_string(),
            ));
        }
        let (messages, content_truncated) =
            match materialize_locators(&self.manifest, &locators, is_cancelled) {
                Ok(page) => page,
                Err(error) => {
                    // A source can be replaced between the pre-read revision
                    // check and locator materialization. Prefer a source
                    // refresh in that case; otherwise retire the disposable
                    // generation because a structurally valid sidecar still
                    // failed to resolve one of its own locators.
                    self.ensure_source_is_current(is_cancelled)?;
                    self.mark_generation_invalid();
                    return Err(TranscriptPageError::RefreshRequired(format!(
                        "Transcript index locator is unusable: {error}"
                    )));
                }
            };
        self.ensure_source_is_current(is_cancelled)?;
        Ok(TranscriptPage {
            generation: self.manifest.generation.clone(),
            page_index,
            total_rows: self.manifest.total_rows,
            message_keys: locators
                .into_iter()
                .map(|locator| locator.message_key)
                .collect(),
            messages,
            has_previous: page_index > 0,
            has_next: page_index + 1 < self.page_count(),
            content_truncated,
        })
    }

    fn ensure_source_is_current(
        &self,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<(), TranscriptPageError> {
        let current_fast = source_fast_revision(
            &self.manifest.provider_id,
            &self.manifest.source_path,
            is_cancelled,
        )
        .map_err(TranscriptPageError::Other)?;
        let mut checkpoint = self.source_checkpoint.lock().map_err(|_| {
            TranscriptPageError::Other(
                "Session transcript revision checkpoint is unavailable".to_string(),
            )
        })?;
        if checkpoint.fast_revision == current_fast {
            return Ok(());
        }

        // The cheap source stamp is global for SQLite and directory-backed
        // stores. If it changes, perform one exact locator comparison for this
        // session. Unrelated writes can then advance the shared checkpoint
        // without rebuilding the index or repeating O(N) work on every page.
        let current = stable_source_revision(
            &self.manifest.provider_id,
            &self.manifest.source_path,
            is_cancelled,
        )
        .map_err(TranscriptPageError::Other)?;
        if current.locators != self.manifest.source_locator_fingerprint {
            return Err(TranscriptPageError::RefreshRequired(
                "Session transcript changed; refreshing its index".to_string(),
            ));
        }
        checkpoint.fast_revision = current.fast.clone();
        drop(checkpoint);
        self._scope_lease
            .persist_validated_fast_revision(&self.manifest.generation, &current.fast);
        Ok(())
    }

    fn mark_generation_invalid(&self) {
        if let Err(error) =
            create_private_marker(&self.generation_dir.join(INVALID_GENERATION_FILE))
        {
            log::debug!(
                "[TRANSCRIPT-INDEX] failed to mark invalid generation {}: {error}",
                self.manifest.generation
            );
        }
    }

    pub(crate) fn locate_message_key(
        &self,
        message_key: &str,
    ) -> Result<Option<usize>, TranscriptPageError> {
        if message_key.is_empty() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(
            &self.db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|error| {
            self.mark_generation_invalid();
            TranscriptPageError::RefreshRequired(format!(
                "Failed to open transcript identity index: {error}"
            ))
        })?;
        let ordinal = conn
            .query_row(
                "SELECT ordinal FROM entries WHERE message_key = ?1 ORDER BY ordinal LIMIT 1",
                [message_key],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| {
                self.mark_generation_invalid();
                TranscriptPageError::RefreshRequired(format!(
                    "Failed to locate transcript message identity: {error}"
                ))
            })?;
        ordinal
            .map(|ordinal| {
                usize::try_from(ordinal)
                    .map(|ordinal| ordinal.saturating_sub(1))
                    .map_err(|_| {
                        TranscriptPageError::RefreshRequired(
                            "Transcript message ordinal exceeds this platform".to_string(),
                        )
                    })
            })
            .transpose()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct TranscriptManifest {
    format_version: u32,
    generation: String,
    provider_id: String,
    source_path: String,
    source_fast_revision: String,
    source_locator_fingerprint: String,
    total_rows: usize,
}

#[derive(Debug)]
struct SourceCheckpoint {
    fast_revision: String,
}

#[derive(Debug, Clone)]
struct SourceRevision {
    fast: String,
    locators: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurrentPointer {
    format_version: u32,
    generation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_fast_revision: Option<String>,
}

#[derive(Debug)]
struct Locator {
    kind: i64,
    text: String,
    offset: i64,
    length: i64,
    message_key: String,
}

#[derive(Debug)]
struct Candidate {
    sort_primary: i64,
    sort_tie: String,
    kind: i64,
    text: String,
    offset: i64,
    length: i64,
    message_key: String,
}

enum BuildAttempt {
    Ready(Box<TranscriptReader>),
    SourceChanged,
    BudgetExceeded,
}

#[derive(Debug, Clone)]
struct TranscriptIndexStore {
    root: PathBuf,
}

struct ScopeCacheEntry {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
    purge_requested: bool,
}

impl TranscriptIndexStore {
    fn open() -> Result<Self, String> {
        let config_dir = resolve_config_dir_without_following_user_symlinks(&get_app_config_dir())
            .map_err(|error| error.to_string())?;
        Self::open_at(&config_dir)
    }

    fn open_at(config_dir: &Path) -> Result<Self, String> {
        let root = config_dir.join(ROOT_DIR);
        create_private_dir(&root)?;
        // Refuse a redirected cache root before any SQLite or lock file can be
        // created beneath it. Production paths also pass through the managed
        // storage component walk in `create_private_dir`.
        validate_private_dir(&root)?;
        let store = Self { root };
        store.cleanup_requested_scopes();
        Ok(store)
    }

    fn open_or_build(
        &self,
        provider_id: &str,
        source_path: &str,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<TranscriptReader, String> {
        validate_source(provider_id, source_path)?;
        let scope = self.scope_dir(provider_id, source_path);
        let fast_revision = source_fast_revision(provider_id, source_path, is_cancelled)?;
        let current = {
            // Global collection takes the exclusive side of this lock. Holding
            // the shared side closes the gap between scope lookup/creation and
            // acquiring the scope/build lease used after this function returns.
            let _root_lock = FileLock::shared(&self.root.join(ROOT_LOCK_FILE))?;
            create_private_dir(&scope)?;
            if scope.join(PURGE_SCOPE_FILE).exists() {
                return Err("Session transcript cache is being retired".to_string());
            }
            self.open_current(provider_id, source_path, &fast_revision, is_cancelled)?
        };
        if let Some(reader) = current {
            self.enforce_cache_budget(&scope);
            return Ok(reader);
        }

        let build_result = (|| -> Result<TranscriptReader, String> {
            // Builds for one physical transcript are serialized across
            // processes. The retry never waits while holding the shared root
            // lock: a completed builder may need the exclusive side for cache
            // collection before releasing this scope lock.
            let build_lock = self.acquire_scope_build_lock(&scope, is_cancelled)?;

            let fast_revision = source_fast_revision(provider_id, source_path, is_cancelled)?;
            {
                let _root_lock = FileLock::shared(&self.root.join(ROOT_LOCK_FILE))?;
                if let Some(reader) =
                    self.open_current(provider_id, source_path, &fast_revision, is_cancelled)?
                {
                    return Ok(reader);
                }
            }

            // Only one transcript index may grow at a time. Existing readers
            // remain fully concurrent; serialization exists solely so the
            // global byte budget can reserve space without another build
            // consuming it between measurement and publication.
            let _cache_build_lock = FileLock::exclusive_cancellable(
                &self.root.join(CACHE_BUILD_LOCK_FILE),
                is_cancelled,
            )?;
            let mut requested_budget =
                INITIAL_TRANSCRIPT_BUILD_BYTES.min(MAX_TRANSCRIPT_CACHE_BYTES);
            let mut source_retries = 0_usize;
            loop {
                if source_retries >= MAX_SOURCE_STABILITY_ATTEMPTS {
                    return Err(
                        "Session transcript kept changing while its index was built".to_string()
                    );
                }
                let build_budget =
                    self.reserve_build_budget(&scope, requested_budget, is_cancelled)?;
                if build_budget < MIN_TRANSCRIPT_BUILD_BYTES {
                    return Err(
                        "Session transcript index cache has no bounded build capacity".to_string(),
                    );
                }
                let attempt = {
                    // Prevent a deletion/collector from redirecting this scope
                    // while SQLite and its publication pointer are active.
                    let _root_lock = FileLock::shared(&self.root.join(ROOT_LOCK_FILE))?;
                    if scope.join(PURGE_SCOPE_FILE).exists() {
                        return Err("Session transcript cache is being retired".to_string());
                    }
                    let fast_revision =
                        source_fast_revision(provider_id, source_path, is_cancelled)?;
                    self.build(
                        provider_id,
                        source_path,
                        fast_revision,
                        build_budget,
                        is_cancelled,
                    )?
                };
                match attempt {
                    BuildAttempt::Ready(reader) => {
                        drop(build_lock);
                        return Ok(*reader);
                    }
                    BuildAttempt::SourceChanged => {
                        source_retries = source_retries.saturating_add(1);
                    }
                    BuildAttempt::BudgetExceeded => {
                        if build_budget < requested_budget
                            || requested_budget >= MAX_TRANSCRIPT_CACHE_BYTES
                        {
                            return Err(format!(
                                "Session transcript index exceeds the bounded {} MiB cache budget",
                                MAX_TRANSCRIPT_CACHE_BYTES / (1024 * 1024)
                            ));
                        }
                        requested_budget = requested_budget
                            .saturating_mul(2)
                            .min(MAX_TRANSCRIPT_CACHE_BYTES);
                    }
                }
            }
        })();

        if build_result.is_err() {
            self.cleanup_empty_scope(&scope);
        }
        let reader = build_result?;
        // Run collection for reused generations too. This closes the recovery
        // gap after a crash between pointer publication and post-build GC.
        self.enforce_cache_budget(&scope);
        Ok(reader)
    }

    fn acquire_scope_build_lock(
        &self,
        scope: &Path,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<FileLock, String> {
        loop {
            if is_cancelled() {
                return Err("Session message index was cancelled".to_string());
            }
            let root_lock = FileLock::shared(&self.root.join(ROOT_LOCK_FILE))?;
            create_private_dir(scope)?;
            if scope.join(PURGE_SCOPE_FILE).exists() {
                return Err("Session transcript cache is being retired".to_string());
            }
            match FileLock::try_exclusive(&scope.join(BUILD_LOCK_FILE))? {
                Some(build_lock) => {
                    drop(root_lock);
                    return Ok(build_lock);
                }
                None => {
                    drop(root_lock);
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    fn open_current(
        &self,
        provider_id: &str,
        source_path: &str,
        fast_revision: &str,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Option<TranscriptReader>, String> {
        if is_cancelled() {
            return Err("Session message index was cancelled".to_string());
        }
        let scope = self.scope_dir(provider_id, source_path);
        if scope.join(PURGE_SCOPE_FILE).exists() {
            return Ok(None);
        }
        let pointer_path = scope.join(CURRENT_FILE);
        if !is_private_regular_file(&pointer_path) {
            return Ok(None);
        }
        let _scope_lock = FileLock::shared(&scope.join(SCOPE_LOCK_FILE))?;
        let pointer: CurrentPointer = match read_json_limited(&pointer_path, MAX_POINTER_BYTES) {
            Ok(pointer) => pointer,
            Err(error) => {
                log::debug!(
                    "[TRANSCRIPT-INDEX] ignoring unusable pointer {}: {error}",
                    pointer_path.display()
                );
                return Ok(None);
            }
        };
        if pointer.format_version != FORMAT_VERSION || !valid_generation(&pointer.generation) {
            return Ok(None);
        }
        let generation_dir = scope.join(&pointer.generation);
        if validate_private_dir(&generation_dir).is_err() {
            return Ok(None);
        }
        if generation_dir.join(INVALID_GENERATION_FILE).exists() {
            return Ok(None);
        }
        let manifest_path = generation_dir.join(MANIFEST_FILE);
        let manifest: TranscriptManifest =
            match read_json_limited(&manifest_path, MAX_MANIFEST_BYTES) {
                Ok(manifest) => manifest,
                Err(error) => {
                    log::debug!(
                        "[TRANSCRIPT-INDEX] ignoring unusable manifest {}: {error}",
                        manifest_path.display()
                    );
                    return Ok(None);
                }
            };
        if manifest.format_version != FORMAT_VERSION
            || manifest.generation != pointer.generation
            || manifest.provider_id != provider_id
            || manifest.source_path != source_path
            || !is_private_regular_file(&generation_dir.join(INDEX_FILE))
        {
            return Ok(None);
        }
        let validated_fast_revision = pointer
            .source_fast_revision
            .as_deref()
            .unwrap_or(&manifest.source_fast_revision);
        let accepted_fast_revision = if validated_fast_revision == fast_revision {
            fast_revision.to_string()
        } else {
            let current = stable_source_revision(provider_id, source_path, is_cancelled)?;
            if current.locators != manifest.source_locator_fingerprint {
                return Ok(None);
            }
            current.fast
        };
        let db_path = generation_dir.join(INDEX_FILE);
        if let Err(error) = validate_index_header(&db_path, manifest.total_rows) {
            log::debug!(
                "[TRANSCRIPT-INDEX] ignoring unusable index {}: {error}",
                db_path.display()
            );
            return Ok(None);
        }
        let scope_lease = Arc::new(ScopeLease::acquire(&self.root, &scope)?);
        let lease = Arc::new(FileLock::shared(
            &generation_dir.join(GENERATION_LEASE_FILE),
        )?);
        drop(_scope_lock);
        if validated_fast_revision != accepted_fast_revision {
            self.persist_validated_fast_revision(
                &scope,
                &pointer.generation,
                &accepted_fast_revision,
            );
        }
        Ok(Some(TranscriptReader {
            db_path,
            generation_dir,
            manifest,
            _lease: lease,
            _scope_lease: scope_lease,
            source_checkpoint: Arc::new(Mutex::new(SourceCheckpoint {
                fast_revision: accepted_fast_revision,
            })),
        }))
    }

    fn build(
        &self,
        provider_id: &str,
        source_path: &str,
        initial_fast_revision: String,
        max_index_bytes: u64,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<BuildAttempt, String> {
        let scope = self.scope_dir(provider_id, source_path);
        create_private_dir(&scope)?;
        let generation = new_generation();
        let generation_dir = scope.join(&generation);
        create_private_dir(&generation_dir)?;
        let result = (|| {
            let db_path = generation_dir.join(INDEX_FILE);
            let mut conn = Connection::open_with_flags(
                &db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                    | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )
            .map_err(|error| format!("Failed to create transcript index: {error}"))?;
            restrict_private_file(&db_path)?;
            configure_index_size_limit(&conn, max_index_bytes)?;
            initialize_index_schema(&conn)?;
            let mut locator_fingerprint = LocatorFingerprint::default();
            {
                let tx = conn
                    .transaction()
                    .map_err(|error| format!("Failed to start transcript index build: {error}"))?;
                {
                    let mut insert = tx
                        .prepare(
                            "INSERT INTO staging (
                                sort_primary, sort_tie, locator_kind, locator_text,
                                locator_offset, locator_length, message_key
                             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        )
                        .map_err(|error| {
                            map_build_sqlite_error(
                                "Failed to prepare transcript index insertion",
                                error,
                            )
                        })?;
                    let mut push = |candidate: Candidate| -> Result<(), String> {
                        if is_cancelled() {
                            return Err("Session message index was cancelled".to_string());
                        }
                        locator_fingerprint.add(&candidate);
                        insert
                            .execute(params![
                                candidate.sort_primary,
                                candidate.sort_tie,
                                candidate.kind,
                                candidate.text,
                                candidate.offset,
                                candidate.length,
                                candidate.message_key,
                            ])
                            .map_err(|error| {
                                map_build_sqlite_error("Failed to insert transcript locator", error)
                            })?;
                        Ok(())
                    };
                    index_provider(provider_id, source_path, is_cancelled, &mut push)?;
                }
                tx.commit().map_err(|error| {
                    map_build_sqlite_error("Failed to commit transcript index staging rows", error)
                })?;
            }
            if is_cancelled() {
                return Err("Session message index was cancelled".to_string());
            }
            super::providers::utils::with_sqlite_cancellation(&conn, is_cancelled, || {
                conn.execute_batch(
                    "BEGIN IMMEDIATE;
                         INSERT INTO entries (
                            locator_kind, locator_text, locator_offset, locator_length,
                            message_key
                         )
                         SELECT locator_kind, locator_text, locator_offset, locator_length,
                                message_key
                         FROM staging
                         ORDER BY sort_primary ASC, sort_tie ASC, insertion ASC;
                         DROP TABLE staging;
                         COMMIT;",
                )
            })
            .map_err(|error| {
                map_build_sqlite_error("Failed to finalize transcript index", error)
            })?;
            let total_i64: i64 = conn
                .query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))
                .map_err(|error| format!("Failed to count transcript index rows: {error}"))?;
            let total_rows = usize::try_from(total_i64)
                .map_err(|_| "Transcript row count exceeds this platform".to_string())?;
            if total_rows != locator_fingerprint.count() {
                return Err("Transcript locator fingerprint count is inconsistent".to_string());
            }
            conn.execute_batch("PRAGMA optimize;").map_err(|error| {
                map_build_sqlite_error("Failed to optimize transcript index", error)
            })?;
            drop(conn);

            let built_locators = locator_fingerprint.finish(provider_id, source_path);
            let post_build_fast = source_fast_revision(provider_id, source_path, is_cancelled)?;
            let accepted_revision = if post_build_fast == initial_fast_revision {
                SourceRevision {
                    fast: post_build_fast,
                    locators: built_locators,
                }
            } else {
                // A concurrent global SQLite write or source replacement may
                // have advanced the cheap stamp. Compare the exact current
                // locator set once; content-only/unrelated changes can reuse
                // this build, while boundary changes restart it.
                let current = stable_source_revision(provider_id, source_path, is_cancelled)?;
                if current.locators != built_locators {
                    return Ok(BuildAttempt::SourceChanged);
                }
                current
            };
            let manifest = TranscriptManifest {
                format_version: FORMAT_VERSION,
                generation: generation.clone(),
                provider_id: provider_id.to_string(),
                source_path: source_path.to_string(),
                source_fast_revision: accepted_revision.fast.clone(),
                source_locator_fingerprint: accepted_revision.locators,
                total_rows,
            };
            let manifest_path = generation_dir.join(MANIFEST_FILE);
            write_json_file(&manifest_path, &manifest).map_err(|error| error.to_string())?;
            restrict_private_file(&manifest_path)?;
            let scope_lease = Arc::new(ScopeLease::acquire(&self.root, &scope)?);
            let lease_path = generation_dir.join(GENERATION_LEASE_FILE);
            let lease = Arc::new(FileLock::shared(&lease_path)?);
            {
                let _scope_lock = FileLock::exclusive(&scope.join(SCOPE_LOCK_FILE))?;
                let _ = fs::remove_file(scope.join(PURGE_SCOPE_FILE));
                let pointer_path = scope.join(CURRENT_FILE);
                write_json_file(
                    &pointer_path,
                    &CurrentPointer {
                        format_version: FORMAT_VERSION,
                        generation: generation.clone(),
                        source_fast_revision: Some(accepted_revision.fast.clone()),
                    },
                )
                .map_err(|error| error.to_string())?;
                restrict_private_file(&pointer_path)?;
                self.cleanup_unleased_generations(&scope, &generation);
            }
            Ok(BuildAttempt::Ready(Box::new(TranscriptReader {
                manifest,
                db_path,
                generation_dir: generation_dir.clone(),
                _lease: lease,
                _scope_lease: scope_lease,
                source_checkpoint: Arc::new(Mutex::new(SourceCheckpoint {
                    fast_revision: accepted_revision.fast,
                })),
            })))
        })();
        if !matches!(&result, Ok(BuildAttempt::Ready(_))) {
            let _ = fs::remove_dir_all(&generation_dir);
        }
        match result {
            Err(error) if error == BUILD_BUDGET_EXCEEDED_ERROR => Ok(BuildAttempt::BudgetExceeded),
            result => result,
        }
    }

    fn cleanup_unleased_generations(&self, scope: &Path, current: &str) {
        let Ok(entries) = fs::read_dir(scope) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name == current || !valid_generation(name) {
                continue;
            }
            let path = entry.path();
            if entry
                .file_type()
                .ok()
                .is_none_or(|file_type| !file_type.is_dir() || file_type.is_symlink())
            {
                continue;
            }
            let lease_path = path.join(GENERATION_LEASE_FILE);
            match FileLock::try_exclusive(&lease_path) {
                Ok(Some(lease)) => {
                    // The caller still owns the exclusive scope lock, so no
                    // new reader can race in after this probe. Release the file
                    // handle before removal for Windows compatibility.
                    drop(lease);
                    if let Err(error) = fs::remove_dir_all(&path) {
                        log::debug!(
                            "[TRANSCRIPT-INDEX] failed to retire {}: {error}",
                            path.display()
                        );
                    }
                }
                Ok(None) => {}
                Err(error) => log::debug!(
                    "[TRANSCRIPT-INDEX] failed to inspect lease {}: {error}",
                    path.display()
                ),
            }
        }
    }

    fn cleanup_requested_scopes(&self) {
        let Ok(_root_lock) = FileLock::exclusive(&self.root.join(ROOT_LOCK_FILE)) else {
            return;
        };
        let mut retired = Vec::new();
        let Ok(entries) = fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !valid_scope_name(name) {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() || !entry.path().join(PURGE_SCOPE_FILE).exists() {
                continue;
            }
            if let Some(path) = self.try_quarantine_scope(&entry.path()) {
                retired.push(path);
            }
        }
        drop(_root_lock);
        remove_quarantined_scopes(retired);
    }

    fn enforce_cache_budget(&self, active_scope: &Path) {
        self.enforce_cache_budget_with_limits(
            active_scope,
            MAX_CACHED_TRANSCRIPT_SCOPES,
            MAX_TRANSCRIPT_CACHE_BYTES,
        );
    }

    fn reserve_build_budget(
        &self,
        active_scope: &Path,
        requested_bytes: u64,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<u64, String> {
        if is_cancelled() {
            return Err("Session message index was cancelled".to_string());
        }
        let root_lock = FileLock::exclusive(&self.root.join(ROOT_LOCK_FILE))?;
        let entries = fs::read_dir(&self.root)
            .map_err(|error| format!("Failed to inspect transcript cache budget: {error}"))?;
        let mut scopes = Vec::new();
        let mut retired = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if name.starts_with(".gc-") && file_type.is_dir() {
                retired.push(entry.path());
                continue;
            }
            if !valid_scope_name(name) || !file_type.is_dir() {
                continue;
            }
            let path = entry.path();
            let modified = fs::metadata(path.join(CURRENT_FILE))
                .and_then(|metadata| metadata.modified())
                .or_else(|_| entry.metadata().and_then(|metadata| metadata.modified()))
                .unwrap_or(SystemTime::UNIX_EPOCH);
            scopes.push(ScopeCacheEntry {
                purge_requested: path.join(PURGE_SCOPE_FILE).exists(),
                bytes: directory_size_no_symlinks(&path),
                path,
                modified,
            });
        }
        scopes.sort_by_key(|scope| (!scope.purge_requested, scope.modified));
        let mut retained_bytes = scopes
            .iter()
            .fold(0_u64, |total, scope| total.saturating_add(scope.bytes));
        let requested_bytes = requested_bytes.min(MAX_TRANSCRIPT_CACHE_BYTES);
        for scope in scopes {
            let fits = retained_bytes
                .saturating_add(TRANSCRIPT_BUILD_METADATA_RESERVE)
                .saturating_add(requested_bytes)
                <= MAX_TRANSCRIPT_CACHE_BYTES;
            if fits {
                break;
            }
            if scope.path == active_scope {
                continue;
            }
            if let Some(path) = self.try_quarantine_scope(&scope.path) {
                retained_bytes = retained_bytes.saturating_sub(scope.bytes);
                retired.push(path);
            }
        }
        drop(root_lock);
        remove_quarantined_scopes(retired);
        if is_cancelled() {
            return Err("Session message index was cancelled".to_string());
        }
        Ok(MAX_TRANSCRIPT_CACHE_BYTES
            .saturating_sub(retained_bytes)
            .saturating_sub(TRANSCRIPT_BUILD_METADATA_RESERVE)
            .min(requested_bytes))
    }

    fn cleanup_empty_scope(&self, scope: &Path) {
        let retired = {
            let Ok(_root_lock) = FileLock::exclusive(&self.root.join(ROOT_LOCK_FILE)) else {
                return;
            };
            if !scope.is_dir()
                || is_private_regular_file(&scope.join(CURRENT_FILE))
                || fs::read_dir(scope).ok().is_some_and(|entries| {
                    entries.flatten().any(|entry| {
                        entry.file_type().ok().is_some_and(|file_type| {
                            file_type.is_dir()
                                && entry.file_name().to_str().is_some_and(valid_generation)
                        })
                    })
                })
            {
                None
            } else {
                self.try_quarantine_scope(scope)
            }
        };
        if let Some(path) = retired {
            remove_quarantined_scopes(vec![path]);
        }
    }

    fn enforce_cache_budget_with_limits(
        &self,
        active_scope: &Path,
        max_scopes: usize,
        max_bytes: u64,
    ) {
        let Ok(_root_lock) = FileLock::exclusive(&self.root.join(ROOT_LOCK_FILE)) else {
            return;
        };
        let Ok(entries) = fs::read_dir(&self.root) else {
            return;
        };
        let mut scopes = Vec::new();
        let mut stale_quarantine = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if name.starts_with(".gc-") && file_type.is_dir() {
                stale_quarantine.push(entry.path());
                continue;
            }
            if !valid_scope_name(name) || !file_type.is_dir() {
                continue;
            }
            let path = entry.path();
            let bytes = directory_size_no_symlinks(&path);
            let modified = fs::metadata(path.join(CURRENT_FILE))
                .and_then(|metadata| metadata.modified())
                .or_else(|_| entry.metadata().and_then(|metadata| metadata.modified()))
                .unwrap_or(SystemTime::UNIX_EPOCH);
            scopes.push(ScopeCacheEntry {
                purge_requested: path.join(PURGE_SCOPE_FILE).exists(),
                path,
                bytes,
                modified,
            });
        }
        scopes.sort_by_key(|scope| (!scope.purge_requested, scope.modified));
        let mut count = scopes.len();
        let mut bytes = scopes
            .iter()
            .fold(0_u64, |total, scope| total.saturating_add(scope.bytes));
        let mut retired = stale_quarantine;
        for scope in scopes {
            let over_budget = count > max_scopes || bytes > max_bytes;
            if !scope.purge_requested && !over_budget {
                break;
            }
            if scope.path == active_scope {
                // A reader necessarily holds the active scope lease while this
                // method runs. If that scope alone cannot fit, leave a durable
                // retirement request; the last reader release schedules the
                // same non-blocking quarantine path used by explicit deletion.
                if !scope.purge_requested && (scope.bytes > max_bytes || max_scopes == 0) {
                    if let Err(error) = create_private_marker(&scope.path.join(PURGE_SCOPE_FILE)) {
                        log::debug!(
                            "[TRANSCRIPT-INDEX] failed to mark oversized active scope {}: {error}",
                            scope.path.display()
                        );
                    }
                }
                continue;
            }
            if let Some(path) = self.try_quarantine_scope(&scope.path) {
                count = count.saturating_sub(1);
                bytes = bytes.saturating_sub(scope.bytes);
                retired.push(path);
            }
        }
        drop(_root_lock);
        remove_quarantined_scopes(retired);
    }

    fn cleanup_requested_scope(&self, scope: &Path) {
        if !scope.join(PURGE_SCOPE_FILE).is_file() {
            return;
        }
        let retired = {
            let Ok(_root_lock) = FileLock::exclusive(&self.root.join(ROOT_LOCK_FILE)) else {
                return;
            };
            self.try_quarantine_scope(scope)
        };
        if let Some(path) = retired {
            remove_quarantined_scopes(vec![path]);
        }
    }

    fn persist_validated_fast_revision(&self, scope: &Path, generation: &str, revision: &str) {
        let pointer_path = scope.join(CURRENT_FILE);
        let result = (|| -> Result<(), String> {
            let _scope_lock = FileLock::exclusive(&scope.join(SCOPE_LOCK_FILE))?;
            if scope.join(PURGE_SCOPE_FILE).exists() {
                return Ok(());
            }
            let mut pointer: CurrentPointer = read_json_limited(&pointer_path, MAX_POINTER_BYTES)?;
            if pointer.format_version != FORMAT_VERSION || pointer.generation != generation {
                return Ok(());
            }
            if pointer.source_fast_revision.as_deref() == Some(revision) {
                return Ok(());
            }
            pointer.source_fast_revision = Some(revision.to_string());
            write_json_file(&pointer_path, &pointer).map_err(|error| error.to_string())?;
            restrict_private_file(&pointer_path)
        })();
        if let Err(error) = result {
            // This is only a cross-open optimization. The immutable manifest
            // and exact locator comparison remain authoritative on failure.
            log::debug!(
                "[TRANSCRIPT-INDEX] failed to persist source revision for {}: {error}",
                scope.display()
            );
        }
    }

    fn purge_scope(&self, provider_id: &str, source_path: &str) {
        let scope = self.scope_dir(provider_id, source_path);
        {
            // Builders/readers take the shared side before resolving a scope.
            // Mark under the exclusive side so a new build cannot observe the
            // old scope and then remove the deletion marker at publication.
            let Ok(_root_lock) = FileLock::exclusive(&self.root.join(ROOT_LOCK_FILE)) else {
                return;
            };
            if validate_private_dir(&scope).is_err() {
                return;
            }
            if let Err(error) = create_private_marker(&scope.join(PURGE_SCOPE_FILE)) {
                log::debug!(
                    "[TRANSCRIPT-INDEX] failed to mark deleted transcript cache {}: {error}",
                    scope.display()
                );
                return;
            }
        }
        self.cleanup_requested_scope(&scope);
        // A live detail reader may still own the scope lease. The marker makes
        // every future store open retry collection before the scope can be
        // reused, so the cache remains disposable without blocking deletion.
    }

    /// Caller owns the exclusive root lock. New readers/builders take its
    /// shared side before touching a scope, while existing work is represented
    /// by the two non-blocking locks below.
    fn try_quarantine_scope(&self, scope: &Path) -> Option<PathBuf> {
        let build = FileLock::try_exclusive(&scope.join(BUILD_LOCK_FILE))
            .ok()
            .flatten()?;
        let lease = FileLock::try_exclusive(&scope.join(SCOPE_LEASE_FILE))
            .ok()
            .flatten()?;
        drop(lease);
        drop(build);
        let quarantine = self
            .root
            .join(format!(".gc-{}", uuid::Uuid::new_v4().simple()));
        match fs::rename(scope, &quarantine) {
            Ok(()) => Some(quarantine),
            Err(error) => {
                log::debug!(
                    "[TRANSCRIPT-INDEX] failed to quarantine {}: {error}",
                    scope.display()
                );
                None
            }
        }
    }

    fn scope_dir(&self, provider_id: &str, source_path: &str) -> PathBuf {
        let mut hasher = Sha256::new();
        hasher.update(provider_id.as_bytes());
        hasher.update([0]);
        hasher.update(source_path.as_bytes());
        self.root.join(format!("{:x}", hasher.finalize()))
    }
}

pub(crate) fn purge_deleted_transcript_cache(provider_id: &str, source_path: &str) {
    match TranscriptIndexStore::open() {
        Ok(store) => store.purge_scope(provider_id, source_path),
        Err(error) => log::debug!(
            "[TRANSCRIPT-INDEX] failed to open cache for deleted transcript purge: {error}"
        ),
    }
}

/// Open or refresh the provider-neutral index, then return the first logical
/// page so a newly-opened detail view follows the transcript's reading order.
pub(crate) fn open_transcript_cancellable(
    provider_id: &str,
    source_path: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(TranscriptReader, TranscriptPage), String> {
    open_transcript_page_cancellable(provider_id, source_path, Some(0), is_cancelled)
}

/// Reopen a changed/corrupt transcript near the page the user was trying to
/// visit. The hint is clamped against the newly-published logical size.
pub(crate) fn open_transcript_page_cancellable(
    provider_id: &str,
    source_path: &str,
    page_hint: Option<usize>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(TranscriptReader, TranscriptPage), String> {
    open_transcript_pages_cancellable(
        provider_id,
        source_path,
        page_hint,
        None,
        None,
        is_cancelled,
    )
    .map(|(reader, page, _)| (reader, page))
}

/// Refresh one source generation and return both the viewport-owning page and,
/// when different, the originally requested page. This lets a speculative
/// prefetch become a real page crossing while the worker is refreshing stale
/// source state, without either losing the crossing or jumping the viewport.
pub(crate) fn open_transcript_pages_cancellable(
    provider_id: &str,
    source_path: &str,
    primary_hint: Option<usize>,
    primary_message_key: Option<&str>,
    secondary_hint: Option<usize>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(TranscriptReader, TranscriptPage, Option<TranscriptPage>), String> {
    validate_source(provider_id, source_path)?;
    let store = TranscriptIndexStore::open()?;
    for _ in 0..MAX_SOURCE_STABILITY_ATTEMPTS {
        let reader = store.open_or_build(provider_id, source_path, is_cancelled)?;
        let anchored_page = match primary_message_key {
            Some(message_key) => match reader.locate_message_key(message_key) {
                Ok(absolute) => absolute.map(|absolute| absolute / TRANSCRIPT_PAGE_SIZE),
                Err(error) if error.requires_refresh() => continue,
                Err(error) => return Err(error.to_string()),
            },
            None => None,
        };
        let page_index = anchored_page
            .or(primary_hint)
            .unwrap_or_else(|| reader.page_count().saturating_sub(1))
            .min(reader.page_count().saturating_sub(1));
        let primary = match reader.load_page(page_index, is_cancelled) {
            Ok(page) => page,
            Err(error) if error.requires_refresh() => continue,
            Err(error) => return Err(error.to_string()),
        };
        let secondary_index = secondary_hint
            .map(|page| page.min(reader.page_count().saturating_sub(1)))
            .filter(|page| *page != page_index);
        let secondary = if let Some(page) = secondary_index {
            match reader.load_page(page, is_cancelled) {
                Ok(page) => Some(page),
                Err(error) if error.requires_refresh() => continue,
                Err(error) => return Err(error.to_string()),
            }
        } else {
            None
        };
        return Ok((reader, primary, secondary));
    }
    Err("Session transcript kept changing while its page was loaded".to_string())
}

#[cfg(test)]
pub(crate) fn open_transcript_at(
    config_dir: &Path,
    provider_id: &str,
    source_path: &str,
) -> Result<(TranscriptReader, TranscriptPage), String> {
    let store = TranscriptIndexStore::open_at(config_dir)?;
    for _ in 0..MAX_SOURCE_STABILITY_ATTEMPTS {
        let reader = store.open_or_build(provider_id, source_path, &|| false)?;
        let page_index = reader.page_count().saturating_sub(1);
        match reader.load_page(page_index, &|| false) {
            Ok(page) => return Ok((reader, page)),
            Err(error) if error.requires_refresh() => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("Session transcript kept changing while its page was loaded".to_string())
}

fn configure_index_size_limit(conn: &Connection, max_bytes: u64) -> Result<(), String> {
    let page_size: u64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|error| format!("Failed to inspect transcript index page size: {error}"))?;
    if page_size == 0 {
        return Err("Transcript index reported an invalid page size".to_string());
    }
    let max_pages = max_bytes / page_size;
    if max_pages == 0 {
        return Err(BUILD_BUDGET_EXCEEDED_ERROR.to_string());
    }
    conn.pragma_update(None, "max_page_count", max_pages)
        .map_err(|error| map_build_sqlite_error("Failed to limit transcript index size", error))
}

fn map_build_sqlite_error(context: &str, error: rusqlite::Error) -> String {
    if matches!(
        &error,
        rusqlite::Error::SqliteFailure(details, _)
            if details.code == rusqlite::ErrorCode::DiskFull
    ) {
        BUILD_BUDGET_EXCEEDED_ERROR.to_string()
    } else {
        format!("{context}: {error}")
    }
}

fn initialize_index_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         PRAGMA temp_store = FILE;
         PRAGMA auto_vacuum = FULL;
         CREATE TABLE staging (
            insertion INTEGER PRIMARY KEY AUTOINCREMENT,
            sort_primary INTEGER NOT NULL,
            sort_tie TEXT NOT NULL,
            locator_kind INTEGER NOT NULL,
            locator_text TEXT NOT NULL,
            locator_offset INTEGER NOT NULL,
            locator_length INTEGER NOT NULL,
            message_key TEXT NOT NULL
         );
         CREATE INDEX staging_order
             ON staging(sort_primary, sort_tie, insertion);
         CREATE TABLE entries (
            ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
            locator_kind INTEGER NOT NULL,
            locator_text TEXT NOT NULL,
            locator_offset INTEGER NOT NULL,
            locator_length INTEGER NOT NULL,
            message_key TEXT NOT NULL
         );
         CREATE INDEX entries_message_key ON entries(message_key);",
    )
    .map_err(|error| map_build_sqlite_error("Failed to initialize transcript index schema", error))
}

/// Cheap structural validation for a disposable sidecar. This deliberately
/// avoids `integrity_check` or a full row count on every detail open: page reads
/// still validate exact cardinality, while the header probe catches missing
/// schema, non-SQLite bytes, and a truncated ordinal tail in O(log N).
fn validate_index_header(path: &Path, expected_rows: usize) -> Result<(), String> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|error| format!("Failed to open transcript index: {error}"))?;
    let max_ordinal: Option<i64> = conn
        .query_row("SELECT MAX(ordinal) FROM entries", [], |row| row.get(0))
        .map_err(|error| format!("Failed to inspect transcript index: {error}"))?;
    let actual_rows = max_ordinal
        .map(usize::try_from)
        .transpose()
        .map_err(|_| "Transcript index ordinal exceeds this platform".to_string())?
        .unwrap_or(0);
    if actual_rows != expected_rows {
        return Err(format!(
            "Transcript index tail is inconsistent: expected {expected_rows}, found {actual_rows}"
        ));
    }
    Ok(())
}

fn index_provider(
    provider_id: &str,
    source_path: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    push: &mut dyn FnMut(Candidate) -> Result<(), String>,
) -> Result<(), String> {
    if provider_id == "opencode" && source_path.starts_with("sqlite:") {
        return index_opencode_sqlite(source_path, is_cancelled, push);
    }
    if provider_id == "hermes" && source_path.starts_with("sqlite:") {
        return index_hermes_sqlite(source_path, is_cancelled, push);
    }
    let path = Path::new(source_path);
    match provider_id {
        "codex" | "claude" | "openclaw" | "hermes" => {
            index_jsonl(provider_id, path, is_cancelled, push)
        }
        "gemini" => index_gemini(path, is_cancelled, push),
        "opencode" => index_opencode_files(path, is_cancelled, push),
        _ => Err(format!("Unsupported provider: {provider_id}")),
    }
}

fn index_jsonl(
    provider_id: &str,
    path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    push: &mut dyn FnMut(Candidate) -> Result<(), String>,
) -> Result<(), String> {
    let file =
        File::open(path).map_err(|error| format!("Failed to open session transcript: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut record = Vec::with_capacity(8 * 1024);
    let mut record_start = 0_u64;
    let mut position = 0_u64;
    let mut sequence = 0_i64;
    let mut oversized = false;
    loop {
        if is_cancelled() {
            return Err("Session message index was cancelled".to_string());
        }
        let buffer = reader
            .fill_buf()
            .map_err(|error| format!("Failed to read session transcript: {error}"))?;
        if buffer.is_empty() {
            if position > record_start {
                index_jsonl_record(
                    provider_id,
                    path,
                    record_start,
                    position - record_start,
                    &record,
                    oversized,
                    &mut sequence,
                    is_cancelled,
                    push,
                )?;
            }
            break;
        }
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(buffer.len(), |index| index + 1);
        let content_len = take.saturating_sub(usize::from(newline.is_some()));
        if !oversized {
            if record.len().saturating_add(content_len) <= MAX_INDEXABLE_RECORD_BYTES {
                record.extend_from_slice(&buffer[..content_len]);
            } else {
                record.clear();
                oversized = true;
            }
        }
        reader.consume(take);
        position = position.saturating_add(take as u64);
        if newline.is_some() {
            let length = position.saturating_sub(record_start).saturating_sub(1);
            index_jsonl_record(
                provider_id,
                path,
                record_start,
                length,
                &record,
                oversized,
                &mut sequence,
                is_cancelled,
                push,
            )?;
            record.clear();
            oversized = false;
            record_start = position;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn index_jsonl_record(
    provider_id: &str,
    path: &Path,
    offset: u64,
    length: u64,
    record: &[u8],
    oversized: bool,
    sequence: &mut i64,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    push: &mut dyn FnMut(Candidate) -> Result<(), String>,
) -> Result<(), String> {
    if length == 0 {
        return Ok(());
    }
    let is_message = if oversized {
        probe_oversized_json_message(provider_id, path, offset, length, is_cancelled)?
    } else {
        std::str::from_utf8(record)
            .ok()
            .and_then(|line| parse_jsonl_message(provider_id, line))
            .is_some()
    };
    if !is_message {
        return Ok(());
    }
    let message_key = if oversized {
        hash_file_range_identity(provider_id, path, offset, length, is_cancelled)?
    } else {
        message_identity(provider_id, record)
    };
    let kind = if oversized {
        LOCATOR_RAW_JSONL
    } else {
        LOCATOR_JSONL
    };
    push(Candidate {
        sort_primary: *sequence,
        sort_tie: String::new(),
        kind,
        // The manifest already owns the common source path. Keeping it out of
        // every locator avoids duplicating it in both staging and final tables.
        text: String::new(),
        offset: u64_to_i64(offset)?,
        length: u64_to_i64(length)?,
        message_key,
    })?;
    *sequence = sequence.saturating_add(1);
    Ok(())
}

fn index_gemini(
    path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    push: &mut dyn FnMut(Candidate) -> Result<(), String>,
) -> Result<(), String> {
    scan_named_json_array(path, "messages", is_cancelled, &mut |element| {
        let is_message = if element.oversized {
            probe_oversized_json_message(
                "gemini",
                path,
                element.offset,
                element.length,
                is_cancelled,
            )?
        } else {
            serde_json::from_slice::<Value>(&element.bytes)
                .ok()
                .and_then(|value| gemini::parse_transcript_message(&value))
                .is_some()
        };
        if !is_message {
            return Ok(());
        }
        let message_key = if element.oversized {
            hash_file_range_identity("gemini", path, element.offset, element.length, is_cancelled)?
        } else {
            message_identity("gemini", &element.bytes)
        };
        push(Candidate {
            sort_primary: element.sequence,
            sort_tie: String::new(),
            kind: if element.oversized {
                LOCATOR_RAW_GEMINI
            } else {
                LOCATOR_GEMINI
            },
            text: String::new(),
            offset: u64_to_i64(element.offset)?,
            length: u64_to_i64(element.length)?,
            message_key,
        })
    })
}

fn index_opencode_files(
    message_dir: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    push: &mut dyn FnMut(Candidate) -> Result<(), String>,
) -> Result<(), String> {
    if !message_dir.is_dir() {
        return Err(format!(
            "Message directory not found: {}",
            message_dir.display()
        ));
    }
    let mut stack = vec![message_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if is_cancelled() {
            return Err("Session message index was cancelled".to_string());
        }
        let entries = fs::read_dir(&dir)
            .map_err(|error| format!("Failed to read OpenCode message directory: {error}"))?;
        for entry in entries {
            if is_cancelled() {
                return Err("Session message index was cancelled".to_string());
            }
            let entry =
                entry.map_err(|error| format!("Failed to read OpenCode message entry: {error}"))?;
            let file_type = entry
                .file_type()
                .map_err(|error| format!("Failed to inspect OpenCode message entry: {error}"))?;
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file()
                || path.extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let metadata = entry
                .metadata()
                .map_err(|error| format!("Failed to inspect OpenCode message entry: {error}"))?;
            if metadata.len() > MAX_INDEXABLE_RECORD_BYTES as u64 {
                let Some((message_id, created_at)) =
                    probe_oversized_opencode_header(&path, is_cancelled)?
                else {
                    continue;
                };
                let path_text = path.to_string_lossy().into_owned();
                let message_key = message_identity("opencode-file", message_id.as_bytes());
                push(Candidate {
                    sort_primary: created_at,
                    sort_tie: message_id,
                    kind: LOCATOR_OPENCODE_FILE,
                    text: path_text,
                    offset: 0,
                    length: 0,
                    message_key,
                })?;
                continue;
            }
            let data = read_file_bounded(&path, MAX_INDEXABLE_RECORD_BYTES, is_cancelled)?;
            let value: Value = match serde_json::from_slice(&data) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let Some(message_id) = value.get("id").and_then(Value::as_str) else {
                continue;
            };
            if message_id.len() > super::SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES {
                continue;
            }
            let created_at = value
                .get("time")
                .and_then(|time| time.get("created"))
                .and_then(super::providers::utils::parse_timestamp_to_ms)
                .unwrap_or(0);
            let message_key = message_identity("opencode-file", message_id.as_bytes());
            push(Candidate {
                sort_primary: created_at,
                sort_tie: message_id.to_string(),
                kind: LOCATOR_OPENCODE_FILE,
                text: path.to_string_lossy().into_owned(),
                offset: 0,
                length: 0,
                message_key,
            })?;
        }
    }
    Ok(())
}

fn index_opencode_sqlite(
    source: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    push: &mut dyn FnMut(Candidate) -> Result<(), String>,
) -> Result<(), String> {
    let (db_path, session_id) = opencode::parse_sqlite_source(source)
        .ok_or_else(|| format!("Invalid SQLite source reference: {source}"))?;
    let conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Failed to open OpenCode database: {error}"))?;
    super::providers::utils::with_sqlite_cancellation(&conn, is_cancelled, || {
        let mut stmt = conn
            .prepare(
                "SELECT rowid
                 FROM message
                 WHERE session_id = ?1
                 ORDER BY time_created ASC, id ASC",
            )
            .map_err(|error| format!("Failed to prepare OpenCode transcript index: {error}"))?;
        let rows = stmt
            .query_map([session_id], |row| row.get::<_, i64>(0))
            .map_err(|error| format!("Failed to query OpenCode transcript index: {error}"))?;
        let mut sequence = 0_i64;
        for row in rows {
            if is_cancelled() {
                return Err("Session message index was cancelled".to_string());
            }
            let rowid =
                row.map_err(|error| format!("Failed to decode OpenCode message row: {error}"))?;
            push(Candidate {
                sort_primary: sequence,
                sort_tie: String::new(),
                kind: LOCATOR_OPENCODE_SQLITE,
                text: String::new(),
                offset: rowid,
                length: 0,
                message_key: message_identity("opencode-sqlite", &rowid.to_le_bytes()),
            })?;
            sequence = sequence.saturating_add(1);
        }
        Ok(())
    })
}

fn index_hermes_sqlite(
    source: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    push: &mut dyn FnMut(Candidate) -> Result<(), String>,
) -> Result<(), String> {
    let (db_path, session_id) = hermes::parse_sqlite_source(source)
        .ok_or_else(|| format!("Invalid SQLite source reference: {source}"))?;
    let conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Failed to open Hermes database: {error}"))?;
    super::providers::utils::with_sqlite_cancellation(&conn, is_cancelled, || {
        let mut stmt = conn
            .prepare(
                "SELECT rowid
                 FROM messages
                 WHERE session_id = ?1
                 ORDER BY created_at ASC, rowid ASC",
            )
            .map_err(|error| format!("Failed to prepare Hermes transcript index: {error}"))?;
        let rows = stmt
            .query_map([session_id], |row| row.get::<_, i64>(0))
            .map_err(|error| format!("Failed to query Hermes transcript index: {error}"))?;
        let mut sequence = 0_i64;
        for row in rows {
            if is_cancelled() {
                return Err("Session message index was cancelled".to_string());
            }
            let rowid =
                row.map_err(|error| format!("Failed to decode Hermes message row: {error}"))?;
            push(Candidate {
                sort_primary: sequence,
                sort_tie: String::new(),
                kind: LOCATOR_HERMES_SQLITE,
                text: String::new(),
                offset: rowid,
                length: 0,
                message_key: message_identity("hermes-sqlite", &rowid.to_le_bytes()),
            })?;
            sequence = sequence.saturating_add(1);
        }
        Ok(())
    })
}

fn materialize_locators(
    manifest: &TranscriptManifest,
    locators: &[Locator],
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(Vec<SessionMessage>, bool), String> {
    if locators
        .first()
        .is_some_and(|locator| locator.kind == LOCATOR_OPENCODE_SQLITE)
    {
        let rowids = locators
            .iter()
            .map(|locator| locator.offset)
            .collect::<Vec<_>>();
        let (messages, truncated) = opencode::load_transcript_sqlite_messages(
            &manifest.source_path,
            &rowids,
            is_cancelled,
        )?;
        return Ok(complete_optional_page(messages, locators.len(), truncated));
    }
    if locators
        .first()
        .is_some_and(|locator| locator.kind == LOCATOR_HERMES_SQLITE)
    {
        let rowids = locators
            .iter()
            .map(|locator| locator.offset)
            .collect::<Vec<_>>();
        let (messages, truncated) =
            hermes::load_transcript_sqlite_messages(&manifest.source_path, &rowids, is_cancelled)?;
        return Ok(complete_optional_page(messages, locators.len(), truncated));
    }

    let mut messages = Vec::with_capacity(locators.len());
    let mut truncated = false;
    let mut open_file: Option<(PathBuf, File)> = None;
    for locator in locators {
        if is_cancelled() {
            return Err("Session message page was cancelled".to_string());
        }
        let outcome = match locator.kind {
            LOCATOR_JSONL | LOCATOR_RAW_JSONL | LOCATOR_GEMINI | LOCATOR_RAW_GEMINI => {
                let path = if locator.text.is_empty() {
                    PathBuf::from(&manifest.source_path)
                } else {
                    PathBuf::from(&locator.text)
                };
                if open_file
                    .as_ref()
                    .is_none_or(|(open_path, _)| open_path != &path)
                {
                    open_file = Some((
                        path.clone(),
                        File::open(&path).map_err(|error| {
                            format!(
                                "Failed to open transcript source {}: {error}",
                                path.display()
                            )
                        })?,
                    ));
                }
                let (_, file) = open_file.as_mut().expect("installed above");
                materialize_file_range(&manifest.provider_id, file, locator)?
            }
            LOCATOR_OPENCODE_FILE => {
                opencode::load_transcript_file_message(Path::new(&locator.text), is_cancelled)?
            }
            _ => return Err("Transcript index contains an unsupported locator".to_string()),
        };
        let (message, shortened) = outcome;
        truncated |= shortened || message.is_none();
        messages.push(message.unwrap_or_else(unavailable_message));
    }
    Ok((messages, truncated))
}

fn materialize_file_range(
    provider_id: &str,
    file: &mut File,
    locator: &Locator,
) -> Result<(Option<SessionMessage>, bool), String> {
    let offset = u64::try_from(locator.offset)
        .map_err(|_| "Transcript locator offset is invalid".to_string())?;
    let length = usize::try_from(locator.length)
        .map_err(|_| "Transcript locator length is invalid".to_string())?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| format!("Failed to seek transcript source: {error}"))?;
    if length > MAX_INDEXABLE_RECORD_BYTES {
        return Ok((
            Some(SessionMessage {
                role: "unknown".to_string(),
                content: "[message body exceeds the bounded preview limit]".to_string(),
                ts: None,
            }),
            true,
        ));
    }
    let mut bytes = vec![0_u8; length];
    file.read_exact(&mut bytes)
        .map_err(|error| format!("Failed to read transcript record: {error}"))?;
    let message = match locator.kind {
        LOCATOR_JSONL => std::str::from_utf8(&bytes)
            .ok()
            .and_then(|line| parse_jsonl_message(provider_id, line)),
        LOCATOR_GEMINI => serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| gemini::parse_transcript_message(&value)),
        LOCATOR_RAW_JSONL | LOCATOR_RAW_GEMINI => Some(SessionMessage {
            role: "unknown".to_string(),
            content: "[message body exceeds the bounded preview limit]".to_string(),
            ts: None,
        }),
        _ => None,
    };
    let mut message = message.unwrap_or_else(unavailable_message);
    let mut truncated = truncate_string_utf8(
        &mut message.role,
        super::SESSION_MESSAGE_PREVIEW_MAX_ROLE_BYTES,
    );
    truncated |= truncate_string_utf8(
        &mut message.content,
        super::SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES,
    );
    Ok((Some(message), truncated))
}

fn complete_optional_page(
    messages: Vec<Option<SessionMessage>>,
    expected: usize,
    mut truncated: bool,
) -> (Vec<SessionMessage>, bool) {
    let mut completed = Vec::with_capacity(expected);
    for message in messages {
        let Some(mut message) = message else {
            completed.push(unavailable_message());
            truncated = true;
            continue;
        };
        truncated |= truncate_string_utf8(
            &mut message.role,
            super::SESSION_MESSAGE_PREVIEW_MAX_ROLE_BYTES,
        );
        truncated |= truncate_string_utf8(
            &mut message.content,
            super::SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES,
        );
        completed.push(message);
    }
    if completed.len() < expected {
        truncated = true;
        completed.resize_with(expected, unavailable_message);
    } else if completed.len() > expected {
        completed.truncate(expected);
        truncated = true;
    }
    (completed, truncated)
}

fn unavailable_message() -> SessionMessage {
    SessionMessage {
        role: "unknown".to_string(),
        content: "[message unavailable]".to_string(),
        ts: None,
    }
}

#[derive(Deserialize)]
struct OversizedJsonEnvelope {
    #[serde(rename = "type")]
    record_type: Option<String>,
    #[serde(rename = "isMeta", default)]
    is_meta: bool,
    message: Option<OversizedJsonMessage>,
    payload: Option<OversizedJsonMessage>,
    role: Option<String>,
    content: Option<IgnoredAny>,
    #[serde(rename = "toolCalls")]
    tool_calls: Option<IgnoredAny>,
}

#[derive(Deserialize)]
struct OversizedJsonMessage {
    #[serde(rename = "type")]
    message_type: Option<String>,
    role: Option<String>,
    content: Option<IgnoredAny>,
    name: Option<IgnoredAny>,
    output: Option<IgnoredAny>,
}

#[derive(Deserialize)]
struct OversizedOpenCodeHeader {
    id: Option<String>,
    time: Option<OversizedOpenCodeTime>,
}

#[derive(Deserialize)]
struct OversizedOpenCodeTime {
    created: Option<Value>,
}

fn probe_oversized_opencode_header(
    path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<(String, i64)>, String> {
    if is_cancelled() {
        return Err("Session message index was cancelled".to_string());
    }
    let file = File::open(path)
        .map_err(|error| format!("Failed to open OpenCode message probe: {error}"))?;
    let reader = CancellableReader {
        inner: file,
        is_cancelled,
    };
    let header: OversizedOpenCodeHeader = match serde_json::from_reader(reader) {
        Ok(header) => header,
        Err(_) if is_cancelled() => {
            return Err("Session message index was cancelled".to_string());
        }
        Err(error) => {
            log::debug!(
                "[TRANSCRIPT-INDEX] skipping invalid oversized OpenCode message {}: {error}",
                path.display()
            );
            return Ok(None);
        }
    };
    let Some(message_id) = header.id.filter(|id| {
        !id.is_empty() && id.len() <= super::SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES
    }) else {
        return Ok(None);
    };
    let created_at = header
        .time
        .as_ref()
        .and_then(|time| time.created.as_ref())
        .and_then(super::providers::utils::parse_timestamp_to_ms)
        .unwrap_or(0);
    Ok(Some((message_id, created_at)))
}

fn probe_oversized_json_message(
    provider_id: &str,
    path: &Path,
    offset: u64,
    length: u64,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<bool, String> {
    if is_cancelled() {
        return Err("Session message index was cancelled".to_string());
    }
    let mut file =
        File::open(path).map_err(|error| format!("Failed to open transcript probe: {error}"))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| format!("Failed to seek transcript probe: {error}"))?;
    let reader = CancellableReader {
        inner: file.take(length),
        is_cancelled,
    };
    let envelope: OversizedJsonEnvelope = match serde_json::from_reader(reader) {
        Ok(envelope) => envelope,
        Err(_error) if is_cancelled() => {
            return Err("Session message index was cancelled".to_string())
        }
        Err(error) => {
            log::debug!(
                "[TRANSCRIPT-INDEX] skipping invalid oversized {provider_id} record: {error}"
            );
            return Ok(false);
        }
    };
    let valid = match provider_id {
        "codex" => {
            envelope.record_type.as_deref() == Some("response_item")
                && envelope.payload.as_ref().is_some_and(|payload| {
                    matches!(
                        payload.message_type.as_deref(),
                        Some("message") if payload.content.is_some()
                    ) || matches!(
                        payload.message_type.as_deref(),
                        Some("function_call") if payload.name.is_some()
                    ) || matches!(
                        payload.message_type.as_deref(),
                        Some("function_call_output") if payload.output.is_some()
                    )
                })
        }
        "claude" => {
            !envelope.is_meta
                && envelope
                    .message
                    .as_ref()
                    .is_some_and(|message| message.content.is_some())
        }
        "openclaw" => {
            envelope.record_type.as_deref() == Some("message")
                && envelope
                    .message
                    .as_ref()
                    .is_some_and(|message| message.content.is_some())
        }
        "hermes" => {
            if envelope.record_type.as_deref() == Some("message") {
                envelope
                    .message
                    .as_ref()
                    .is_some_and(|message| message.role.is_some() && message.content.is_some())
            } else {
                envelope.role.is_some() && envelope.content.is_some()
            }
        }
        "gemini" => {
            matches!(envelope.record_type.as_deref(), Some("user" | "gemini"))
                && (envelope.content.is_some() || envelope.tool_calls.is_some())
        }
        _ => false,
    };
    Ok(valid)
}

struct CancellableReader<'a, R> {
    inner: R,
    is_cancelled: &'a (dyn Fn() -> bool + Sync),
}

impl<R: Read> Read for CancellableReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if (self.is_cancelled)() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "session transcript read cancelled",
            ));
        }
        self.inner.read(buffer)
    }
}

fn parse_jsonl_message(provider_id: &str, line: &str) -> Option<SessionMessage> {
    match provider_id {
        "codex" => codex::parse_transcript_line(line),
        "claude" => claude::parse_transcript_line(line),
        "openclaw" => openclaw::parse_transcript_line(line),
        "hermes" => hermes::parse_transcript_line(line),
        _ => None,
    }
}

fn validate_source(provider_id: &str, source_path: &str) -> Result<(), String> {
    if !matches!(
        provider_id,
        "codex" | "claude" | "opencode" | "openclaw" | "gemini" | "hermes"
    ) {
        return Err(format!("Unsupported provider: {provider_id}"));
    }
    if source_path.trim().is_empty() || source_path.len() > 64 * 1024 {
        return Err("Invalid transcript source".to_string());
    }
    Ok(())
}

/// Cheap O(1) source stamp used on every bounded page read. A mismatch is
/// promoted to the exact locator comparison below, so unrelated SQLite writes
/// cause one scoped scan rather than one scan per page.
fn source_fast_revision(
    provider_id: &str,
    source_path: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<String, String> {
    if is_cancelled() {
        return Err("Session message index was cancelled".to_string());
    }
    let mut hasher = Sha256::new();
    hash_revision_namespace(&mut hasher, provider_id, source_path);
    if provider_id == "opencode" && source_path.starts_with("sqlite:") {
        let (path, _) = opencode::parse_sqlite_source(source_path)
            .ok_or_else(|| "Invalid OpenCode SQLite source".to_string())?;
        hash_sqlite_fast_revision(&mut hasher, &path)?;
    } else if provider_id == "hermes" && source_path.starts_with("sqlite:") {
        let (path, _) = hermes::parse_sqlite_source(source_path)
            .ok_or_else(|| "Invalid Hermes SQLite source".to_string())?;
        hash_sqlite_fast_revision(&mut hasher, &path)?;
    } else {
        let path = Path::new(source_path);
        let metadata = fs::metadata(path).map_err(|error| {
            format!(
                "Failed to fingerprint transcript source {}: {error}",
                path.display()
            )
        })?;
        hash_path_metadata(&mut hasher, path, &metadata)?;

        // File-backed OpenCode stores immutable message headers directly in one
        // directory per session. Membership changes advance this directory;
        // mutable message parts are content-only, read live, and do not affect
        // locator boundaries.
    }
    if is_cancelled() {
        return Err("Session message index was cancelled".to_string());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn stable_source_revision(
    provider_id: &str,
    source_path: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<SourceRevision, String> {
    for _ in 0..MAX_SOURCE_STABILITY_ATTEMPTS {
        let before = source_fast_revision(provider_id, source_path, is_cancelled)?;
        let locators = source_locator_fingerprint(provider_id, source_path, is_cancelled)?;
        let after = source_fast_revision(provider_id, source_path, is_cancelled)?;
        if before == after {
            return Ok(SourceRevision {
                fast: after,
                locators,
            });
        }
    }
    Err("Session transcript kept changing while its revision was inspected".to_string())
}

fn source_locator_fingerprint(
    provider_id: &str,
    source_path: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<String, String> {
    let mut fingerprint = LocatorFingerprint::default();
    index_provider(provider_id, source_path, is_cancelled, &mut |candidate| {
        fingerprint.add(&candidate);
        Ok(())
    })?;
    if is_cancelled() {
        return Err("Session message index was cancelled".to_string());
    }
    Ok(fingerprint.finish(provider_id, source_path))
}

#[derive(Default)]
struct LocatorFingerprint {
    count: u64,
    sums: [u64; 4],
    xors: [u64; 4],
}

impl LocatorFingerprint {
    fn count(&self) -> usize {
        usize::try_from(self.count).unwrap_or(usize::MAX)
    }

    fn add(&mut self, candidate: &Candidate) {
        let mut hasher = Sha256::new();
        hasher.update(candidate.sort_primary.to_le_bytes());
        hash_length_prefixed(&mut hasher, candidate.sort_tie.as_bytes());
        hasher.update(candidate.kind.to_le_bytes());
        hash_length_prefixed(&mut hasher, candidate.text.as_bytes());
        hasher.update(candidate.offset.to_le_bytes());
        hasher.update(candidate.length.to_le_bytes());
        let digest = hasher.finalize();
        for lane in 0..4 {
            let start = lane * 8;
            let value = u64::from_le_bytes(
                digest[start..start + 8]
                    .try_into()
                    .expect("SHA-256 lane has eight bytes"),
            );
            self.sums[lane] = self.sums[lane].wrapping_add(value);
            self.xors[lane] ^= value;
        }
        self.count = self.count.wrapping_add(1);
    }

    fn finish(self, provider_id: &str, source_path: &str) -> String {
        let mut hasher = Sha256::new();
        hash_revision_namespace(&mut hasher, provider_id, source_path);
        hasher.update(self.count.to_le_bytes());
        for value in self.sums.into_iter().chain(self.xors) {
            hasher.update(value.to_le_bytes());
        }
        format!("{:x}", hasher.finalize())
    }
}

fn hash_revision_namespace(hasher: &mut Sha256, provider_id: &str, source_path: &str) {
    hash_length_prefixed(hasher, provider_id.as_bytes());
    hash_length_prefixed(hasher, source_path.as_bytes());
}

fn hash_length_prefixed(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn message_identity(namespace: &str, bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hash_length_prefixed(&mut hasher, namespace.as_bytes());
    hash_length_prefixed(&mut hasher, bytes);
    format!("{:x}", hasher.finalize())
}

fn hash_file_range_identity(
    namespace: &str,
    path: &Path,
    offset: u64,
    length: u64,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("Failed to open transcript identity source: {error}"))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| format!("Failed to seek transcript identity source: {error}"))?;
    let mut reader = file.take(length);
    let mut hasher = Sha256::new();
    hash_length_prefixed(&mut hasher, namespace.as_bytes());
    hasher.update(length.to_le_bytes());
    let mut remaining = length;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        if is_cancelled() {
            return Err("Session message index was cancelled".to_string());
        }
        let take = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| "Transcript identity range exceeds this platform".to_string())?;
        let read = reader
            .read(&mut buffer[..take])
            .map_err(|error| format!("Failed to hash transcript identity: {error}"))?;
        if read == 0 {
            return Err("Transcript identity source ended early".to_string());
        }
        hasher.update(&buffer[..read]);
        remaining = remaining.saturating_sub(read as u64);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_sqlite_fast_revision(hasher: &mut Sha256, path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "Failed to fingerprint transcript database {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "Transcript database is not a regular file: {}",
            path.display()
        ));
    }
    hash_path_metadata(hasher, path, &metadata)?;
    for suffix in ["-wal", "-journal"] {
        hash_optional_path_metadata(hasher, &path_with_suffix(path, suffix))?;
    }
    Ok(())
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn hash_optional_path_metadata(hasher: &mut Sha256, path: &Path) -> Result<(), String> {
    match fs::metadata(path) {
        Ok(metadata) => {
            hasher.update([1]);
            hash_path_metadata(hasher, path, &metadata)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            hasher.update([0]);
            Ok(())
        }
        Err(error) => Err(format!(
            "Failed to fingerprint transcript companion {}: {error}",
            path.display()
        )),
    }
}

fn hash_path_metadata(
    hasher: &mut Sha256,
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), String> {
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(format!(
            "Transcript source is not a regular file or directory: {}",
            path.display()
        ));
    }
    hasher.update([u8::from(metadata.is_dir())]);
    hasher.update(metadata.len().to_le_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hasher.update(metadata.dev().to_le_bytes());
        hasher.update(metadata.ino().to_le_bytes());
        hasher.update(metadata.ctime().to_le_bytes());
        hasher.update(metadata.ctime_nsec().to_le_bytes());
    }
    #[cfg(not(unix))]
    {
        let created = metadata
            .created()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .unwrap_or_default();
        hasher.update(created.as_secs().to_le_bytes());
        hasher.update(created.subsec_nanos().to_le_bytes());
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .unwrap_or_default();
    hasher.update(modified.as_secs().to_le_bytes());
    hasher.update(modified.subsec_nanos().to_le_bytes());
    Ok(())
}

struct JsonArrayElement {
    sequence: i64,
    offset: u64,
    length: u64,
    bytes: Vec<u8>,
    oversized: bool,
}

/// Locate elements of one top-level named JSON array without retaining the
/// containing document. It tracks JSON strings/escapes and nested containers,
/// so commas inside content never split an element.
fn scan_named_json_array(
    path: &Path,
    key: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    on_element: &mut dyn FnMut(JsonArrayElement) -> Result<(), String>,
) -> Result<(), String> {
    let file =
        File::open(path).map_err(|error| format!("Failed to open Gemini transcript: {error}"))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut position = 0_u64;
    let mut root_depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut capture_key = false;
    let mut key_bytes = Vec::new();
    let mut root_expects_key = false;
    let mut matched_key = false;
    let mut awaiting_array = false;
    let mut in_target = false;
    let mut found_target = false;
    let mut element_start = None;
    let mut element_last_non_ws = 0_u64;
    let mut element_bytes = Vec::with_capacity(8 * 1024);
    let mut element_oversized = false;
    let mut nested_depth = 0_i32;
    let mut sequence = 0_i64;

    loop {
        if is_cancelled() {
            return Err("Session message index was cancelled".to_string());
        }
        let buffer = reader
            .fill_buf()
            .map_err(|error| format!("Failed to scan Gemini transcript: {error}"))?;
        if buffer.is_empty() {
            break;
        }
        let chunk_len = buffer.len();
        for byte in buffer.iter().copied() {
            let byte_position = position;
            position = position.saturating_add(1);

            if in_target {
                if element_start.is_none() {
                    if byte.is_ascii_whitespace() || byte == b',' {
                        continue;
                    }
                    if byte == b']' {
                        in_target = false;
                        continue;
                    }
                    element_start = Some(byte_position);
                    element_last_non_ws = position;
                    nested_depth = 0;
                    in_string = false;
                    escaped = false;
                    element_bytes.clear();
                    element_oversized = false;
                }

                let delimiter = !in_string && nested_depth == 0 && matches!(byte, b',' | b']');
                if delimiter {
                    let start = element_start.take().expect("set above");
                    let length = element_last_non_ws.saturating_sub(start);
                    on_element(JsonArrayElement {
                        sequence,
                        offset: start,
                        length,
                        bytes: std::mem::take(&mut element_bytes),
                        oversized: element_oversized,
                    })?;
                    sequence = sequence.saturating_add(1);
                    if byte == b']' {
                        in_target = false;
                    }
                    continue;
                }

                if !element_oversized {
                    if element_bytes.len() < MAX_INDEXABLE_RECORD_BYTES {
                        element_bytes.push(byte);
                    } else {
                        element_bytes.clear();
                        element_oversized = true;
                    }
                }
                if !byte.is_ascii_whitespace() {
                    element_last_non_ws = position;
                }
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        in_string = false;
                    }
                } else {
                    match byte {
                        b'"' => in_string = true,
                        b'{' | b'[' => nested_depth += 1,
                        b'}' | b']' => nested_depth -= 1,
                        _ => {}
                    }
                }
                continue;
            }

            if in_string {
                if escaped {
                    escaped = false;
                    if capture_key && key_bytes.len() <= key.len() {
                        key_bytes.push(byte);
                    }
                } else if byte == b'\\' {
                    escaped = true;
                    if capture_key && key_bytes.len() <= key.len() {
                        key_bytes.push(byte);
                    }
                } else if byte == b'"' {
                    in_string = false;
                    if capture_key && key_bytes == key.as_bytes() {
                        matched_key = true;
                    }
                    capture_key = false;
                } else if capture_key && key_bytes.len() <= key.len() {
                    key_bytes.push(byte);
                }
                continue;
            }

            if awaiting_array {
                if byte.is_ascii_whitespace() {
                    continue;
                }
                if byte == b'[' {
                    in_target = true;
                    found_target = true;
                    awaiting_array = false;
                    continue;
                }
                return Err(format!("Gemini transcript field {key:?} is not an array"));
            }
            if matched_key {
                if byte.is_ascii_whitespace() {
                    continue;
                }
                if byte == b':' {
                    matched_key = false;
                    awaiting_array = true;
                    continue;
                }
                matched_key = false;
            }

            match byte {
                b'{' => {
                    root_depth += 1;
                    if root_depth == 1 {
                        root_expects_key = true;
                    }
                }
                b'}' => root_depth -= 1,
                b',' if root_depth == 1 => root_expects_key = true,
                b':' if root_depth == 1 => root_expects_key = false,
                b'"' => {
                    in_string = true;
                    escaped = false;
                    capture_key = root_depth == 1 && root_expects_key;
                    key_bytes.clear();
                }
                _ => {}
            }
        }
        reader.consume(chunk_len);
    }
    if element_start.is_some() || in_target {
        return Err("Gemini transcript ended inside a messages element".to_string());
    }
    if awaiting_array || !found_target {
        return Err(format!("No {key} array found in Gemini transcript"));
    }
    Ok(())
}

fn read_file_bounded(
    path: &Path,
    max_bytes: usize,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Failed to inspect {}: {error}", path.display()))?;
    let declared = usize::try_from(metadata.len())
        .map_err(|_| format!("File is too large: {}", path.display()))?;
    if declared > max_bytes {
        return Err(format!(
            "File exceeds the bounded transcript metadata limit: {}",
            path.display()
        ));
    }
    let mut file =
        File::open(path).map_err(|error| format!("Failed to open {}: {error}", path.display()))?;
    let mut bytes = Vec::with_capacity(declared);
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        if is_cancelled() {
            return Err("Session message index was cancelled".to_string());
        }
        let read = file
            .read(&mut chunk)
            .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) > max_bytes {
            return Err(format!(
                "File exceeds the bounded transcript metadata limit: {}",
                path.display()
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok(bytes)
}

#[derive(Debug)]
struct FileLock {
    file: File,
}

impl FileLock {
    fn shared(path: &Path) -> Result<Self, String> {
        let file = open_lock_file(path)?;
        file.lock_shared()
            .map_err(|error| format!("Failed to lock {}: {error}", path.display()))?;
        Ok(Self { file })
    }

    fn exclusive(path: &Path) -> Result<Self, String> {
        let file = open_lock_file(path)?;
        file.lock()
            .map_err(|error| format!("Failed to lock {}: {error}", path.display()))?;
        Ok(Self { file })
    }

    fn exclusive_cancellable(
        path: &Path,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Self, String> {
        let file = open_lock_file(path)?;
        loop {
            if is_cancelled() {
                return Err("Session message index was cancelled".to_string());
            }
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(error) => {
                    let error: std::io::Error = error.into();
                    if error.kind() != std::io::ErrorKind::WouldBlock {
                        return Err(format!("Failed to lock {}: {error}", path.display()));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    fn try_exclusive(path: &Path) -> Result<Option<Self>, String> {
        let file = open_lock_file(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { file })),
            Err(error) => {
                let error: std::io::Error = error.into();
                if error.kind() == std::io::ErrorKind::WouldBlock {
                    Ok(None)
                } else {
                    Err(format!("Failed to lock {}: {error}", path.display()))
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

/// Shared ownership of one transcript cache scope. A purge request can be
/// created while readers are alive; the final release retires it without
/// blocking the UI worker that dropped the reader.
#[derive(Debug)]
struct ScopeLease {
    lock: Option<FileLock>,
    root: PathBuf,
    scope: PathBuf,
}

impl ScopeLease {
    fn acquire(root: &Path, scope: &Path) -> Result<Self, String> {
        Ok(Self {
            lock: Some(FileLock::shared(&scope.join(SCOPE_LEASE_FILE))?),
            root: root.to_path_buf(),
            scope: scope.to_path_buf(),
        })
    }

    fn persist_validated_fast_revision(&self, generation: &str, revision: &str) {
        TranscriptIndexStore {
            root: self.root.clone(),
        }
        .persist_validated_fast_revision(&self.scope, generation, revision);
    }
}

impl Drop for ScopeLease {
    fn drop(&mut self) {
        let purge_requested = self.scope.join(PURGE_SCOPE_FILE).is_file();
        // Release the shared lease before trying to take the exclusive side.
        self.lock.take();
        if !purge_requested {
            return;
        }
        let store = TranscriptIndexStore {
            root: self.root.clone(),
        };
        let scope = self.scope.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("transcript-scope-cleanup".to_string())
            .spawn(move || store.cleanup_requested_scope(&scope))
        {
            // The durable marker remains and the next store open retries it.
            log::debug!("[TRANSCRIPT-INDEX] failed to schedule retired scope cleanup: {error}");
        }
    }
}

fn open_lock_file(path: &Path) -> Result<File, String> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let file = options
            .open(path)
            .map_err(|error| format!("Failed to open lock {}: {error}", path.display()))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("Failed to inspect lock {}: {error}", path.display()))?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(format!(
                "Transcript cache lock is not a private regular file: {}",
                path.display()
            ));
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Failed to secure lock {}: {error}", path.display()))?;
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let file = options
            .open(path)
            .map_err(|error| format!("Failed to open lock {}: {error}", path.display()))?;
        if !file
            .metadata()
            .map_err(|error| format!("Failed to inspect lock {}: {error}", path.display()))?
            .is_file()
        {
            return Err(format!(
                "Transcript cache lock is not a regular file: {}",
                path.display()
            ));
        }
        Ok(file)
    }
}

fn create_private_dir(path: &Path) -> Result<(), String> {
    create_managed_config_dir_all(path).map_err(|error| error.to_string())?;
    validate_private_dir(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let directory = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(path)
            .map_err(|error| format!("Failed to open directory {}: {error}", path.display()))?;
        directory
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Failed to secure {}: {error}", path.display()))?;
    }
    Ok(())
}

fn validate_private_dir(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Failed to inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "Transcript cache path is not a private directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn restrict_private_file(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| format!("Failed to open {}: {error}", path.display()))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("Failed to inspect {}: {error}", path.display()))?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(format!(
                "Transcript cache file is not a private regular file: {}",
                path.display()
            ));
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Failed to secure {}: {error}", path.display()))?;
    }
    Ok(())
}

fn create_private_marker(path: &Path) -> Result<(), String> {
    let file = open_lock_file(path)?;
    file.set_len(0)
        .map_err(|error| format!("Failed to write marker {}: {error}", path.display()))
}

fn is_private_regular_file(path: &Path) -> bool {
    let Some(metadata) = fs::symlink_metadata(path).ok() else {
        return false;
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return false;
        }
    }
    true
}

fn valid_scope_name(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn directory_size_no_symlinks(root: &Path) -> u64 {
    let mut total = 0_u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                total = total.saturating_add(entry.metadata().map_or(0, |metadata| metadata.len()));
            }
        }
    }
    total
}

fn remove_quarantined_scopes(paths: Vec<PathBuf>) {
    for path in paths {
        if fs::symlink_metadata(&path)
            .ok()
            .is_none_or(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
        {
            continue;
        }
        if let Err(error) = fs::remove_dir_all(&path) {
            log::debug!(
                "[TRANSCRIPT-INDEX] failed to remove quarantine {}: {error}",
                path.display()
            );
        }
    }
}

fn read_json_limited<T: for<'de> Deserialize<'de>>(
    path: &Path,
    max_bytes: u64,
) -> Result<T, String> {
    let file = open_private_read_file(path)?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Failed to inspect {}: {error}", path.display()))?;
    if metadata.len() > max_bytes {
        return Err(format!(
            "Transcript index file is too large: {}",
            path.display()
        ));
    }
    serde_json::from_reader(file)
        .map_err(|error| format!("Failed to parse {}: {error}", path.display()))
}

fn open_private_read_file(path: &Path) -> Result<File, String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let file = options
            .open(path)
            .map_err(|error| format!("Failed to open {}: {error}", path.display()))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("Failed to inspect {}: {error}", path.display()))?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(format!(
                "Transcript cache file is not a private regular file: {}",
                path.display()
            ));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let file = options
            .open(path)
            .map_err(|error| format!("Failed to open {}: {error}", path.display()))?;
        if !file
            .metadata()
            .map_err(|error| format!("Failed to inspect {}: {error}", path.display()))?
            .is_file()
        {
            return Err(format!(
                "Transcript cache file is not a regular file: {}",
                path.display()
            ));
        }
        Ok(file)
    }
}

fn new_generation() -> String {
    format!(
        "gen-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    )
}

fn valid_generation(value: &str) -> bool {
    value.starts_with("gen-")
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn u64_to_i64(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "Transcript locator exceeds SQLite limits".to_string())
}

fn usize_to_i64(value: usize) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "Transcript page exceeds SQLite limits".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn codex_line(index: usize) -> String {
        serde_json::json!({
            "type": "response_item",
            "timestamp": index as i64,
            "payload": {
                "type": "message",
                "role": if index.is_multiple_of(2) { "user" } else { "assistant" },
                "content": [{"type": "input_text", "text": format!("message-{index}")}],
            }
        })
        .to_string()
    }

    fn claude_line(index: usize) -> String {
        serde_json::json!({
            "timestamp": index as i64,
            "message": {
                "role": if index.is_multiple_of(2) { "user" } else { "assistant" },
                "content": [{
                    "type": "text",
                    "text": format!("claude-message-{index}"),
                }],
            }
        })
        .to_string()
    }

    #[test]
    fn jsonl_index_exposes_every_page_and_reuses_unchanged_generation() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("session.jsonl");
        let mut body = String::new();
        for index in 0..250 {
            body.push_str(&codex_line(index));
            body.push('\n');
        }
        fs::write(&source, body).expect("write source");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let source = source.to_string_lossy().into_owned();

        let first = store
            .open_or_build("codex", &source, &|| false)
            .expect("build index");
        assert_eq!(first.total_rows(), 250);
        assert_eq!(first.page_count(), 3);
        let newest = first.load_page(2, &|| false).expect("newest page");
        assert_eq!(newest.messages.len(), 50);
        assert_eq!(newest.messages[0].content, "message-200");
        assert_eq!(newest.messages[49].content, "message-249");
        let middle = first.load_page(1, &|| false).expect("middle page");
        assert_eq!(middle.messages.len(), TRANSCRIPT_PAGE_SIZE);
        assert_eq!(middle.messages[0].content, "message-100");
        assert!(middle.has_previous);
        assert!(middle.has_next);

        let reopened = store
            .open_or_build("codex", &source, &|| false)
            .expect("reuse index");
        assert_eq!(reopened.generation(), first.generation());
    }

    #[test]
    fn initial_open_returns_the_first_logical_page() {
        let temp = tempdir().expect("tempdir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let sessions_dir = temp.path().join(".codex").join("sessions");
        fs::create_dir_all(&sessions_dir).expect("create Codex sessions directory");
        let source = sessions_dir.join("session.jsonl");
        let body = (0..205).map(codex_line).collect::<Vec<_>>().join("\n");
        fs::write(&source, body).expect("write source");

        let (reader, page) =
            open_transcript_cancellable("codex", &source.to_string_lossy(), &|| false)
                .expect("open transcript");

        assert_eq!(reader.total_rows(), 205);
        assert_eq!(page.page_index, 0);
        assert_eq!(page.messages.len(), TRANSCRIPT_PAGE_SIZE);
        assert_eq!(page.messages[0].content, "message-0");
        assert_eq!(page.messages[99].content, "message-99");
    }

    #[test]
    fn claude_jsonl_index_exposes_the_complete_transcript() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("session.jsonl");
        let mut body = String::new();
        for index in 0..205 {
            body.push_str(&claude_line(index));
            body.push('\n');
        }
        fs::write(&source, body).expect("write source");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let source = source.to_string_lossy().into_owned();

        let reader = store
            .open_or_build("claude", &source, &|| false)
            .expect("build index");
        assert_eq!(reader.total_rows(), 205);
        assert_eq!(reader.page_count(), 3);

        let first = reader.load_page(0, &|| false).expect("first page");
        assert_eq!(first.messages.len(), TRANSCRIPT_PAGE_SIZE);
        assert_eq!(first.messages[0].role, "user");
        assert_eq!(first.messages[0].content, "claude-message-0");
        assert!(first.has_next);

        let last = reader.load_page(2, &|| false).expect("last page");
        assert_eq!(last.messages.len(), 5);
        assert_eq!(last.messages[4].role, "user");
        assert_eq!(last.messages[4].content, "claude-message-204");
        assert!(last.has_previous);
        assert!(!last.has_next);
    }

    #[test]
    fn changed_source_invalidates_old_reader_and_publishes_a_new_generation() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("session.jsonl");
        fs::write(&source, format!("{}\n", codex_line(0))).expect("write source");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let source_text = source.to_string_lossy().into_owned();
        let old = store
            .open_or_build("codex", &source_text, &|| false)
            .expect("old index");
        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(&source, format!("{}\n{}\n", codex_line(0), codex_line(1)))
            .expect("replace source");
        let new = store
            .open_or_build("codex", &source_text, &|| false)
            .expect("new index");
        assert_ne!(new.generation(), old.generation());
        assert_eq!(new.total_rows(), 2);
        assert!(matches!(
            old.load_page(0, &|| false),
            Err(TranscriptPageError::RefreshRequired(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rename_replace_with_unchanged_locators_reuses_the_live_generation() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("session.jsonl");
        let original = format!("{}\n", codex_line(0));
        let replacement = format!("{}\n", codex_line(2));
        assert_eq!(original.len(), replacement.len());
        fs::write(&source, original).expect("write original source");
        let original_modified = fs::metadata(&source)
            .and_then(|metadata| metadata.modified())
            .expect("original mtime");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let source_text = source.to_string_lossy().into_owned();
        let first = store
            .open_or_build("codex", &source_text, &|| false)
            .expect("first generation");

        let staged = temp.path().join("replacement.jsonl");
        fs::write(&staged, replacement).expect("write replacement");
        File::options()
            .write(true)
            .open(&staged)
            .and_then(|file| {
                file.set_times(std::fs::FileTimes::new().set_modified(original_modified))
            })
            .expect("preserve replacement mtime");
        fs::rename(&staged, &source).expect("replace source by rename");

        let second = store
            .open_or_build("codex", &source_text, &|| false)
            .expect("replacement generation");
        assert_eq!(second.generation(), first.generation());
        assert_eq!(
            second
                .load_page(0, &|| false)
                .expect("replacement page")
                .messages[0]
                .content,
            "message-2"
        );
        assert_eq!(
            first
                .load_page(0, &|| false)
                .expect("old reader resolves the live source")
                .messages[0]
                .content,
            "message-2"
        );
    }

    #[test]
    fn incomplete_middle_page_marks_the_generation_invalid_and_self_heals() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("session.jsonl");
        let body = (0..205).map(codex_line).collect::<Vec<_>>().join("\n");
        fs::write(&source, format!("{body}\n")).expect("write source");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let source_text = source.to_string_lossy().into_owned();
        let first = store
            .open_or_build("codex", &source_text, &|| false)
            .expect("initial index");
        let first_generation = first.generation().to_string();
        Connection::open(&first.db_path)
            .expect("open disposable index")
            .execute("DELETE FROM entries WHERE ordinal = 50", [])
            .expect("remove a middle locator");

        assert!(matches!(
            first.load_page(0, &|| false),
            Err(TranscriptPageError::RefreshRequired(_))
        ));
        assert!(first.generation_dir.join(INVALID_GENERATION_FILE).is_file());
        drop(first);

        let rebuilt = store
            .open_or_build("codex", &source_text, &|| false)
            .expect("rebuild invalid generation");
        assert_ne!(rebuilt.generation(), first_generation);
        assert_eq!(rebuilt.total_rows(), 205);
        assert_eq!(
            rebuilt
                .load_page(0, &|| false)
                .expect("rebuilt page")
                .messages[49]
                .content,
            "message-49"
        );
    }

    #[test]
    fn oversized_records_are_validated_before_they_become_locators() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("session.jsonl");
        let padding = "x".repeat(MAX_INDEXABLE_RECORD_BYTES + 1);
        let metadata = serde_json::json!({
            "type": "session_meta",
            "payload": {"padding": padding},
        })
        .to_string();
        let oversized_message = serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": padding}],
            }
        })
        .to_string();
        let small_message = codex_line(2);
        fs::write(
            &source,
            format!("{metadata}\n{oversized_message}\n{small_message}\n"),
        )
        .expect("write oversized transcript");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let reader = store
            .open_or_build("codex", &source.to_string_lossy(), &|| false)
            .expect("index oversized transcript");
        let page = reader.load_page(0, &|| false).expect("oversized page");

        assert_eq!(reader.total_rows(), 2, "metadata must not become a message");
        assert_eq!(
            page.messages[0].content,
            "[message body exceeds the bounded preview limit]"
        );
        assert_eq!(page.messages[1].content, "message-2");
        assert!(page.content_truncated);
    }

    #[test]
    fn concurrent_openers_converge_on_one_published_generation() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("session.jsonl");
        fs::write(&source, format!("{}\n", codex_line(0))).expect("write source");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let source = Arc::new(source.to_string_lossy().into_owned());
        let barrier = Arc::new(std::sync::Barrier::new(3));

        let open =
            |store: TranscriptIndexStore, source: Arc<String>, barrier: Arc<std::sync::Barrier>| {
                std::thread::spawn(move || {
                    barrier.wait();
                    store
                        .open_or_build("codex", source.as_str(), &|| false)
                        .expect("concurrent open")
                        .generation()
                        .to_string()
                })
            };
        let first = open(store.clone(), Arc::clone(&source), Arc::clone(&barrier));
        let second = open(store, source, Arc::clone(&barrier));
        barrier.wait();

        assert_eq!(
            first.join().expect("first opener"),
            second.join().expect("second opener")
        );
    }

    #[test]
    fn bounded_sqlite_build_removes_a_generation_that_reaches_its_hard_limit() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("session.jsonl");
        let body = (0..4_000).map(codex_line).collect::<Vec<_>>().join("\n");
        fs::write(&source, format!("{body}\n")).expect("write source");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let source_text = source.to_string_lossy().into_owned();
        let fast_revision =
            source_fast_revision("codex", &source_text, &|| false).expect("source revision");
        let scope = store.scope_dir("codex", &source_text);

        let attempt = store
            .build("codex", &source_text, fast_revision, 128 * 1024, &|| false)
            .expect("bounded build result");

        assert!(matches!(attempt, BuildAttempt::BudgetExceeded));
        assert!(!scope.join(CURRENT_FILE).exists());
        let generations = fs::read_dir(&scope)
            .expect("read failed build scope")
            .flatten()
            .filter(|entry| entry.file_name().to_str().is_some_and(valid_generation))
            .count();
        assert_eq!(
            generations, 0,
            "a hard-limit retry must not retain a partial SQLite generation"
        );
    }

    #[test]
    fn failed_initial_build_removes_its_empty_cache_scope() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("session.json");
        fs::write(&source, r#"{"sessionId":"broken","items":[]}"#)
            .expect("write invalid Gemini source");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let source_text = source.to_string_lossy().into_owned();
        let scope = store.scope_dir("gemini", &source_text);

        let error = store
            .open_or_build("gemini", &source_text, &|| false)
            .expect_err("invalid source must fail");

        assert!(error.contains("messages array"));
        assert!(
            !scope.exists(),
            "a failed first build must not leak an empty cache scope"
        );
    }

    #[test]
    fn staging_order_uses_the_bounded_main_database_index() {
        let conn = Connection::open_in_memory().expect("database");
        initialize_index_schema(&conn).expect("schema");
        let mut statement = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT locator_kind, locator_text, locator_offset, locator_length, message_key
                 FROM staging
                 ORDER BY sort_primary ASC, sort_tie ASC, insertion ASC",
            )
            .expect("query plan");
        let plan = statement
            .query_map([], |row| row.get::<_, String>(3))
            .expect("plan rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("decode plan");

        assert!(
            plan.iter().any(|detail| detail.contains("staging_order")),
            "finalization must scan the incrementally-built ordering index: {plan:?}"
        );
        assert!(
            plan.iter()
                .all(|detail| !detail.contains("USE TEMP B-TREE")),
            "finalization must not spill an unbounded temporary sort: {plan:?}"
        );
    }

    #[test]
    fn gemini_array_scanner_pages_large_documents_without_whole_file_limit() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("session.json");
        let messages = (0_usize..205)
            .map(|index| {
                serde_json::json!({
                    "type": if index.is_multiple_of(2) { "user" } else { "gemini" },
                    "content": format!("gemini-{index}"),
                    "timestamp": index,
                })
            })
            .collect::<Vec<_>>();
        fs::write(
            &source,
            serde_json::to_vec(&serde_json::json!({
                "sessionId": "session",
                "messages": messages,
            }))
            .expect("serialize"),
        )
        .expect("write source");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let reader = store
            .open_or_build("gemini", &source.to_string_lossy(), &|| false)
            .expect("index");
        assert_eq!(reader.total_rows(), 205);
        let last = reader.load_page(2, &|| false).expect("last page");
        assert_eq!(last.messages.len(), 5);
        assert_eq!(last.messages[4].content, "gemini-204");
    }

    #[test]
    fn gemini_scanner_rejects_a_missing_or_unterminated_messages_array() {
        let temp = tempdir().expect("tempdir");
        let missing = temp.path().join("missing.json");
        fs::write(&missing, r#"{"sessionId":"x","items":[]}"#).expect("missing fixture");
        let unterminated = temp.path().join("unterminated.json");
        fs::write(
            &unterminated,
            r#"{"messages":[{"type":"user","content":"hello"}"#,
        )
        .expect("unterminated fixture");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");

        assert!(store
            .open_or_build("gemini", &missing.to_string_lossy(), &|| false)
            .expect_err("missing messages must fail")
            .contains("messages array"));
        assert!(store
            .open_or_build("gemini", &unterminated.to_string_lossy(), &|| false)
            .expect_err("unterminated messages must fail")
            .contains("ended inside"));
    }

    #[test]
    fn corrupt_disposable_pointer_is_rebuilt_without_touching_the_source() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("session.jsonl");
        fs::write(&source, format!("{}\n", codex_line(0))).expect("source");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let source_text = source.to_string_lossy().into_owned();
        let first = store
            .open_or_build("codex", &source_text, &|| false)
            .expect("initial index");
        let pointer = store.scope_dir("codex", &source_text).join(CURRENT_FILE);
        fs::write(&pointer, b"{broken").expect("corrupt disposable pointer");

        let rebuilt = store
            .open_or_build("codex", &source_text, &|| false)
            .expect("rebuild corrupt sidecar");

        assert_ne!(rebuilt.generation(), first.generation());
        assert_eq!(
            rebuilt
                .load_page(0, &|| false)
                .expect("rebuilt page")
                .messages[0]
                .content,
            "message-0"
        );
        assert_eq!(
            fs::read_to_string(&source).expect("source remains readable"),
            format!("{}\n", codex_line(0))
        );
    }

    #[test]
    fn corrupt_disposable_index_is_rebuilt_and_sidecar_omits_message_text() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("session.jsonl");
        let secret = "ultra-secret-transcript-token";
        let line = serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": secret}],
            }
        })
        .to_string();
        fs::write(&source, format!("{line}\n")).expect("source");
        let config = temp.path().join("config");
        let store = TranscriptIndexStore::open_at(&config).expect("store");
        let source_text = source.to_string_lossy().into_owned();
        let first = store
            .open_or_build("codex", &source_text, &|| false)
            .expect("initial index");
        let old_generation = first.generation().to_string();
        drop(first);

        let index = store
            .scope_dir("codex", &source_text)
            .join(&old_generation)
            .join(INDEX_FILE);
        fs::write(&index, b"not a sqlite database").expect("corrupt disposable index");
        let rebuilt = store
            .open_or_build("codex", &source_text, &|| false)
            .expect("rebuild corrupt index");
        assert_ne!(rebuilt.generation(), old_generation);
        assert_eq!(
            rebuilt
                .load_page(0, &|| false)
                .expect("rebuilt page")
                .messages[0]
                .content,
            secret
        );

        let locator_conn =
            Connection::open(&rebuilt.db_path).expect("inspect rebuilt locator database");
        let duplicated_paths: i64 = locator_conn
            .query_row(
                "SELECT COUNT(*) FROM entries WHERE locator_text != ''",
                [],
                |row| row.get(0),
            )
            .expect("count duplicated source paths");
        let auto_vacuum: i64 = locator_conn
            .query_row("PRAGMA auto_vacuum", [], |row| row.get(0))
            .expect("read auto-vacuum mode");
        let free_pages: i64 = locator_conn
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))
            .expect("read free-page count");
        assert_eq!(duplicated_paths, 0);
        assert_eq!(
            auto_vacuum, 1,
            "sidecars must reclaim dropped staging pages"
        );
        assert_eq!(
            free_pages, 0,
            "published sidecars must not retain staging pages"
        );

        let mut stack = vec![config];
        while let Some(path) = stack.pop() {
            for entry in fs::read_dir(path).expect("read sidecar directory") {
                let entry = entry.expect("sidecar entry");
                if entry.file_type().expect("entry type").is_dir() {
                    stack.push(entry.path());
                    continue;
                }
                let bytes = fs::read(entry.path()).expect("read sidecar file");
                assert!(
                    !bytes
                        .windows(secret.len())
                        .any(|window| window == secret.as_bytes()),
                    "locator sidecar must not duplicate transcript plaintext"
                );
            }
        }
    }

    #[test]
    fn opencode_file_store_pages_every_message_without_materializing_the_directory() {
        let temp = tempdir().expect("tempdir");
        let storage = temp.path().join("storage");
        let message_dir = storage.join("message").join("ses_1");
        fs::create_dir_all(&message_dir).expect("message directory");
        for index in 0_usize..205 {
            let message_id = format!("msg-{index:03}");
            fs::write(
                message_dir.join(format!("{message_id}.json")),
                serde_json::json!({
                    "id": message_id,
                    "role": if index.is_multiple_of(2) { "user" } else { "assistant" },
                    "time": {"created": index},
                })
                .to_string(),
            )
            .expect("message header");
            let part_dir = storage.join("part").join(format!("msg-{index:03}"));
            fs::create_dir_all(&part_dir).expect("part directory");
            fs::write(
                part_dir.join("part.json"),
                serde_json::json!({
                    "type": "text",
                    "text": format!("opencode-file-{index}"),
                })
                .to_string(),
            )
            .expect("message part");
        }
        fs::write(
            message_dir.join("not-a-message.json"),
            serde_json::json!({
                "kind": "metadata",
                "padding": "x".repeat(MAX_INDEXABLE_RECORD_BYTES + 1),
            })
            .to_string(),
        )
        .expect("oversized metadata");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let reader = store
            .open_or_build("opencode", &message_dir.to_string_lossy(), &|| false)
            .expect("index");
        let first = reader.load_page(0, &|| false).expect("first page");
        let last = reader.load_page(2, &|| false).expect("last page");

        assert_eq!(reader.total_rows(), 205);
        assert_eq!(first.messages[0].content, "opencode-file-0");
        assert_eq!(first.messages[99].content, "opencode-file-99");
        assert_eq!(last.messages.len(), 5);
        assert_eq!(last.messages[4].content, "opencode-file-204");
    }

    #[test]
    fn opencode_part_changes_are_read_live_without_rebuilding_locators() {
        let temp = tempdir().expect("tempdir");
        let storage = temp.path().join("storage");
        let message_dir = storage.join("message").join("ses_1");
        let message_path = message_dir.join("msg_1.json");
        let part_dir = storage.join("part").join("msg_1");
        fs::create_dir_all(&message_dir).expect("message directory");
        fs::create_dir_all(&part_dir).expect("part directory");
        fs::write(
            &message_path,
            serde_json::json!({
                "id": "msg_1",
                "role": "assistant",
                "time": {"created": 1},
            })
            .to_string(),
        )
        .expect("message header");
        let part_path = part_dir.join("part.json");
        fs::write(
            &part_path,
            serde_json::json!({"type": "text", "text": "before"}).to_string(),
        )
        .expect("initial part");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let source = message_dir.to_string_lossy().into_owned();
        let first = store
            .open_or_build("opencode", &source, &|| false)
            .expect("first generation");

        std::thread::sleep(Duration::from_millis(2));
        fs::write(
            &part_path,
            serde_json::json!({"type": "text", "text": "after"}).to_string(),
        )
        .expect("updated part");
        let second = store
            .open_or_build("opencode", &source, &|| false)
            .expect("refreshed generation");

        assert_eq!(second.generation(), first.generation());
        assert_eq!(
            second
                .load_page(0, &|| false)
                .expect("updated page")
                .messages[0]
                .content,
            "after"
        );

        std::thread::sleep(Duration::from_millis(2));
        fs::write(
            message_dir.join("msg_2.json"),
            serde_json::json!({
                "id": "msg_2",
                "role": "user",
                "time": {"created": 2},
            })
            .to_string(),
        )
        .expect("second message header");
        let second_part_dir = storage.join("part").join("msg_2");
        fs::create_dir_all(&second_part_dir).expect("second part directory");
        fs::write(
            second_part_dir.join("part.json"),
            serde_json::json!({"type": "text", "text": "second"}).to_string(),
        )
        .expect("second message part");
        let third = store
            .open_or_build("opencode", &source, &|| false)
            .expect("new locator generation");
        assert_ne!(third.generation(), first.generation());
        assert_eq!(third.total_rows(), 2);
    }

    #[test]
    fn deleted_scope_is_retired_immediately_or_after_its_reader_closes() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("session.jsonl");
        fs::write(&source, format!("{}\n", codex_line(0))).expect("source");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let source_text = source.to_string_lossy().into_owned();
        let reader = store
            .open_or_build("codex", &source_text, &|| false)
            .expect("reader");
        let scope = store.scope_dir("codex", &source_text);

        store.purge_scope("codex", &source_text);
        assert!(scope.join(PURGE_SCOPE_FILE).is_file());
        drop(reader);
        store.cleanup_requested_scopes();
        assert!(!scope.exists());

        let rebuilt = store
            .open_or_build("codex", &source_text, &|| false)
            .expect("rebuilt reader");
        drop(rebuilt);
        store.purge_scope("codex", &source_text);
        assert!(!scope.exists());
    }

    #[test]
    fn cache_budget_keeps_the_active_scope_and_bounds_scope_count() {
        let temp = tempdir().expect("tempdir");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let active = store.root.join(format!("{:064x}", 0));
        for index in 0..=MAX_CACHED_TRANSCRIPT_SCOPES {
            create_private_dir(&store.root.join(format!("{index:064x}")))
                .expect("create cache scope");
        }

        store.enforce_cache_budget(&active);

        let retained = fs::read_dir(&store.root)
            .expect("read cache root")
            .flatten()
            .filter(|entry| {
                entry.file_type().is_ok_and(|file_type| file_type.is_dir())
                    && entry.file_name().to_str().is_some_and(valid_scope_name)
            })
            .count();
        assert!(active.is_dir());
        assert!(retained <= MAX_CACHED_TRANSCRIPT_SCOPES);
    }

    #[test]
    fn oversized_active_scope_is_retired_after_its_last_reader_closes() {
        let temp = tempdir().expect("tempdir");
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let active = store.root.join(format!("{:064x}", 7));
        create_private_dir(&active).expect("active scope");
        fs::write(active.join("payload"), [0_u8; 32]).expect("cache payload");
        let lease = ScopeLease::acquire(&store.root, &active).expect("scope lease");

        store.enforce_cache_budget_with_limits(&active, 128, 1);
        assert!(active.join(PURGE_SCOPE_FILE).is_file());
        assert!(active.is_dir(), "a live scope must not be removed");

        drop(lease);
        for _ in 0..100 {
            if !active.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !active.exists(),
            "the final scope reader should schedule durable cleanup"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cache_root_symlink_is_rejected_without_mutating_its_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let temp = tempdir().expect("tempdir");
        let config = temp.path().join("config");
        let target = temp.path().join("external");
        fs::create_dir_all(&config).expect("config");
        fs::create_dir_all(&target).expect("external target");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))
            .expect("set external permissions");
        symlink(&target, config.join(ROOT_DIR)).expect("redirect cache root");

        assert!(TranscriptIndexStore::open_at(&config).is_err());
        assert_eq!(
            fs::metadata(&target)
                .expect("target metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(fs::read_dir(&target).expect("target contents").count(), 0);
    }

    #[test]
    fn opencode_sqlite_pages_by_bounded_rowid_locators() {
        let temp = tempdir().expect("tempdir");
        let db_path = temp.path().join("opencode.db");
        let mut conn = Connection::open(&db_path).expect("database");
        conn.execute_batch(
            "CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
             );
             CREATE TABLE part (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
             );",
        )
        .expect("schema");
        let tx = conn.transaction().expect("transaction");
        for index in 0_usize..205 {
            let id = format!("msg-{index:03}");
            tx.execute(
                "INSERT INTO message (id, session_id, time_created, data)
                 VALUES (?1, 'ses_1', ?2, ?3)",
                params![
                    id,
                    index as i64,
                    serde_json::json!({
                        "role": if index.is_multiple_of(2) {
                            "user"
                        } else {
                            "assistant"
                        }
                    })
                    .to_string()
                ],
            )
            .expect("message");
            tx.execute(
                "INSERT INTO part (id, session_id, message_id, time_created, data)
                 VALUES (?1, 'ses_1', ?2, ?3, ?4)",
                params![
                    format!("part-{index:03}"),
                    format!("msg-{index:03}"),
                    index as i64,
                    serde_json::json!({
                        "type": "text",
                        "text": format!("opencode-{index}")
                    })
                    .to_string()
                ],
            )
            .expect("part");
        }
        tx.commit().expect("commit");
        drop(conn);
        let source = format!("sqlite:{}:ses_1", db_path.display());
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");

        let reader = store
            .open_or_build("opencode", &source, &|| false)
            .expect("index");
        let last = reader.load_page(2, &|| false).expect("last page");

        assert_eq!(reader.total_rows(), 205);
        assert_eq!(last.messages.len(), 5);
        assert_eq!(last.messages[0].content, "opencode-200");
        assert_eq!(last.messages[4].content, "opencode-204");
    }

    #[test]
    fn sqlite_revision_ignores_unrelated_sessions_and_refreshes_the_target() {
        let temp = tempdir().expect("tempdir");
        let db_path = temp.path().join("opencode.db");
        let conn = Connection::open(&db_path).expect("database");
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                time_updated INTEGER NOT NULL
             );
             CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
             );
             CREATE TABLE part (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
             );
             INSERT INTO session (id, time_updated)
                 VALUES ('ses_target', 1), ('ses_unrelated', 1);
             INSERT INTO message (id, session_id, time_created, data)
                 VALUES ('target-1', 'ses_target', 1, '{\"role\":\"user\"}'),
                        ('unrelated-1', 'ses_unrelated', 1, '{\"role\":\"user\"}');
             INSERT INTO part (id, session_id, message_id, time_created, data)
                 VALUES ('target-part-1', 'ses_target', 'target-1', 1,
                         '{\"type\":\"text\",\"text\":\"target-1\"}'),
                        ('unrelated-part-1', 'ses_unrelated', 'unrelated-1', 1,
                         '{\"type\":\"text\",\"text\":\"unrelated-1\"}');",
        )
        .expect("schema and seed data");

        let source = format!("sqlite:{}:ses_target", db_path.display());
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let first = store
            .open_or_build("opencode", &source, &|| false)
            .expect("first generation");

        conn.execute(
            "INSERT INTO message (id, session_id, time_created, data)
             VALUES ('unrelated-2', 'ses_unrelated', 2, '{\"role\":\"assistant\"}')",
            [],
        )
        .expect("unrelated message");
        conn.execute(
            "INSERT INTO part (id, session_id, message_id, time_created, data)
             VALUES ('unrelated-part-2', 'ses_unrelated', 'unrelated-2', 2,
                     '{\"type\":\"text\",\"text\":\"unrelated-2\"}')",
            [],
        )
        .expect("unrelated part");
        let after_unrelated = store
            .open_or_build("opencode", &source, &|| false)
            .expect("reuse after unrelated write");
        assert_eq!(after_unrelated.generation(), first.generation());
        let current_fast =
            source_fast_revision("opencode", &source, &|| false).expect("current fast revision");
        let pointer: CurrentPointer = read_json_limited(
            &store.scope_dir("opencode", &source).join(CURRENT_FILE),
            MAX_POINTER_BYTES,
        )
        .expect("persisted pointer");
        assert_eq!(
            pointer.source_fast_revision.as_deref(),
            Some(current_fast.as_str()),
            "a verified unrelated write should not force another exact scan on reopen"
        );

        conn.execute(
            "UPDATE part
             SET data = '{\"type\":\"text\",\"text\":\"target-1-updated\"}'
             WHERE id = 'target-part-1'",
            [],
        )
        .expect("target content update");
        let after_content = store
            .open_or_build("opencode", &source, &|| false)
            .expect("reuse after content-only write");
        assert_eq!(after_content.generation(), first.generation());
        assert_eq!(
            after_content
                .load_page(0, &|| false)
                .expect("updated content page")
                .messages[0]
                .content,
            "target-1-updated"
        );

        conn.execute(
            "INSERT INTO message (id, session_id, time_created, data)
             VALUES ('target-2', 'ses_target', 2, '{\"role\":\"assistant\"}')",
            [],
        )
        .expect("target message");
        conn.execute(
            "INSERT INTO part (id, session_id, message_id, time_created, data)
             VALUES ('target-part-2', 'ses_target', 'target-2', 2,
                     '{\"type\":\"text\",\"text\":\"target-2\"}')",
            [],
        )
        .expect("target part");
        let after_target = store
            .open_or_build("opencode", &source, &|| false)
            .expect("refresh after target write");

        assert_ne!(after_target.generation(), first.generation());
        assert_eq!(after_target.total_rows(), 2);
        assert_eq!(
            after_target
                .load_page(0, &|| false)
                .expect("target page")
                .messages
                .last()
                .map(|message| message.content.as_str()),
            Some("target-2")
        );
    }

    #[test]
    fn sqlite_revision_detects_non_extreme_timestamp_reordering() {
        let temp = tempdir().expect("tempdir");
        let db_path = temp.path().join("opencode.db");
        let conn = Connection::open(&db_path).expect("database");
        conn.execute_batch(
            "CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
             );
             CREATE TABLE part (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
             );
             INSERT INTO message (id, session_id, time_created, data) VALUES
                ('msg-1', 'ses_target', 1, '{\"role\":\"user\"}'),
                ('msg-2', 'ses_target', 2, '{\"role\":\"assistant\"}'),
                ('msg-3', 'ses_target', 3, '{\"role\":\"user\"}');
             INSERT INTO part (id, session_id, message_id, time_created, data) VALUES
                ('part-1', 'ses_target', 'msg-1', 1,
                 '{\"type\":\"text\",\"text\":\"one\"}'),
                ('part-2', 'ses_target', 'msg-2', 2,
                 '{\"type\":\"text\",\"text\":\"two\"}'),
                ('part-3', 'ses_target', 'msg-3', 3,
                 '{\"type\":\"text\",\"text\":\"three\"}');",
        )
        .expect("schema and seed data");
        let source = format!("sqlite:{}:ses_target", db_path.display());
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");
        let first = store
            .open_or_build("opencode", &source, &|| false)
            .expect("first generation");

        conn.execute("UPDATE message SET time_created = 4 WHERE id = 'msg-2'", [])
            .expect("reorder middle message");
        let reordered = store
            .open_or_build("opencode", &source, &|| false)
            .expect("reordered generation");
        let page = reordered.load_page(0, &|| false).expect("reordered page");

        assert_ne!(reordered.generation(), first.generation());
        assert_eq!(
            page.messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "three", "two"]
        );
    }

    #[test]
    fn hermes_sqlite_pages_by_ordered_rowid_locators() {
        let temp = tempdir().expect("tempdir");
        let db_path = temp.path().join("hermes.db");
        let mut conn = Connection::open(&db_path).expect("database");
        conn.execute_batch(
            "CREATE TABLE messages (
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL
             );",
        )
        .expect("schema");
        let tx = conn.transaction().expect("transaction");
        for index in 0_usize..205 {
            tx.execute(
                "INSERT INTO messages (session_id, role, content, created_at)
                 VALUES ('session-1', ?1, ?2, ?3)",
                params![
                    if index.is_multiple_of(2) {
                        "user"
                    } else {
                        "assistant"
                    },
                    format!("hermes-{index}"),
                    index as i64
                ],
            )
            .expect("message");
        }
        tx.commit().expect("commit");
        drop(conn);
        let source = format!("sqlite:{}#session-1", db_path.display());
        let store = TranscriptIndexStore::open_at(&temp.path().join("config")).expect("store");

        let reader = store
            .open_or_build("hermes", &source, &|| false)
            .expect("index");
        let first = reader.load_page(0, &|| false).expect("first page");
        let last = reader.load_page(2, &|| false).expect("last page");

        assert_eq!(reader.total_rows(), 205);
        assert_eq!(first.messages[0].content, "hermes-0");
        assert_eq!(first.messages[99].content, "hermes-99");
        assert_eq!(last.messages[4].content, "hermes-204");
    }
}
