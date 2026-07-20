pub mod cache;
#[allow(
    dead_code,
    reason = "page-manifest integration is staged independently from its storage primitive"
)]
pub mod paged_manifest;
pub(crate) mod project_scope;
pub mod providers;
pub(crate) mod query_manifest;
pub mod recent_snapshot;
pub mod scan_cache_store;
pub mod terminal;

use serde::{Deserialize, Serialize};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use providers::{claude, codex, gemini, hermes, openclaw, opencode};
use scan_cache_store::ScanCacheStore;

/// Session metadata as rendered on the Sessions page.
///
/// `Deserialize`/`Default` back the persistent scan cache: metadata is stored as
/// JSON and read back into this struct. `#[serde(default)]` at the container
/// level makes every field tolerant of a missing key, so a cache written by an
/// older build that lacked a field still deserializes (the field defaults). When
/// the shape changes incompatibly, bump `cache::SCAN_CACHE_VERSION` instead of
/// relying on field-level tolerance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionMeta {
    pub provider_id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<i64>,
}

/// Fixed-cost detail preview returned by every session provider.
///
/// The limits are enforced while parsing, before a message is retained. This
/// keeps both worker memory and all subsequent UI filtering/rendering bounded.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessageBatch {
    pub messages: Vec<SessionMessage>,
    pub truncated: bool,
}

impl From<Vec<SessionMessage>> for SessionMessageBatch {
    fn from(messages: Vec<SessionMessage>) -> Self {
        Self {
            messages,
            truncated: false,
        }
    }
}

impl std::ops::Deref for SessionMessageBatch {
    type Target = [SessionMessage];

    fn deref(&self) -> &Self::Target {
        &self.messages
    }
}

pub const SESSION_MESSAGE_PREVIEW_MAX_MESSAGES: usize = 200;
pub const SESSION_MESSAGE_PREVIEW_MAX_BODY_BYTES: usize = 1024 * 1024;
pub const SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES: usize = 16 * 1024;
pub(crate) const SESSION_MESSAGE_PREVIEW_MAX_ROLE_BYTES: usize = 256;

pub(crate) struct SessionMessageBatchBuilder {
    batch: SessionMessageBatch,
    body_bytes: usize,
}

impl SessionMessageBatchBuilder {
    pub(crate) fn new() -> Self {
        Self {
            batch: SessionMessageBatch::default(),
            body_bytes: 0,
        }
    }

    /// Retain one bounded message. `Break` means count or total-body capacity
    /// was reached and the provider must stop reading immediately.
    pub(crate) fn push(&mut self, mut message: SessionMessage) -> ControlFlow<()> {
        if self.is_full() {
            self.batch.truncated = true;
            return ControlFlow::Break(());
        }

        if truncate_string_utf8(&mut message.role, SESSION_MESSAGE_PREVIEW_MAX_ROLE_BYTES) {
            self.batch.truncated = true;
        }
        let remaining = SESSION_MESSAGE_PREVIEW_MAX_BODY_BYTES.saturating_sub(self.body_bytes);
        let content_limit = SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES.min(remaining);
        if truncate_string_utf8(&mut message.content, content_limit) {
            self.batch.truncated = true;
        }
        self.body_bytes = self.body_bytes.saturating_add(message.content.len());
        self.batch.messages.push(message);

        if self.is_full() {
            // We intentionally stop at the boundary rather than probing the
            // rest of a potentially enormous transcript. The preview is
            // conservatively labelled truncated when exact EOF is unknown.
            self.batch.truncated = true;
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }

    pub(crate) fn mark_truncated(&mut self) {
        self.batch.truncated = true;
    }

    pub(crate) fn is_full(&self) -> bool {
        self.batch.messages.len() >= SESSION_MESSAGE_PREVIEW_MAX_MESSAGES
            || self.body_bytes >= SESSION_MESSAGE_PREVIEW_MAX_BODY_BYTES
    }

    pub(crate) fn finish(self) -> SessionMessageBatch {
        self.batch
    }
}

pub(crate) fn truncate_string_utf8(value: &mut String, max_bytes: usize) -> bool {
    if value.len() <= max_bytes {
        return false;
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    true
}

/// Append as much UTF-8 text as fits. Returns true when any input was omitted.
pub(crate) fn append_utf8_bounded(output: &mut String, input: &str, max_bytes: usize) -> bool {
    let remaining = max_bytes.saturating_sub(output.len());
    if input.len() <= remaining {
        output.push_str(input);
        return false;
    }
    let mut end = remaining.min(input.len());
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    output.push_str(&input[..end]);
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct DeleteSessionRequest {
    pub provider_id: String,
    pub session_id: String,
    pub source_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct DeleteSessionOutcome {
    pub provider_id: String,
    pub session_id: String,
    pub source_path: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[allow(dead_code)]
pub fn scan_sessions() -> Vec<SessionMeta> {
    let (r1, r2, r3, r4, r5, r6) = std::thread::scope(|s| {
        let h1 = s.spawn(codex::scan_sessions);
        let h2 = s.spawn(claude::scan_sessions);
        let h3 = s.spawn(opencode::scan_sessions);
        let h4 = s.spawn(openclaw::scan_sessions);
        let h5 = s.spawn(gemini::scan_sessions);
        let h6 = s.spawn(hermes::scan_sessions);
        (
            h1.join().unwrap_or_default(),
            h2.join().unwrap_or_default(),
            h3.join().unwrap_or_default(),
            h4.join().unwrap_or_default(),
            h5.join().unwrap_or_default(),
            h6.join().unwrap_or_default(),
        )
    });

    let mut sessions = Vec::new();
    sessions.extend(r1);
    sessions.extend(r2);
    sessions.extend(r3);
    sessions.extend(r4);
    sessions.extend(r5);
    sessions.extend(r6);

    sort_by_recent(&mut sessions);
    sessions
}

pub fn scan_sessions_for_provider(provider_id: &str) -> Vec<SessionMeta> {
    let mut sessions = scan_sessions_unordered_for_provider(provider_id);

    sort_by_recent(&mut sessions);
    sessions
}

/// Provider scan without an intermediate sort. The all-provider TUI worker
/// moves each provider Vec into one merged Vec and sorts that authoritative
/// result exactly once.
pub(crate) fn scan_sessions_unordered_for_provider(provider_id: &str) -> Vec<SessionMeta> {
    match provider_id {
        "codex" => codex::scan_sessions(),
        "claude" => claude::scan_sessions(),
        "opencode" => opencode::scan_sessions(),
        "openclaw" => openclaw::scan_sessions(),
        "gemini" => gemini::scan_sessions(),
        "hermes" => hermes::scan_sessions(),
        _ => Vec::new(),
    }
}

/// Sort sessions most-recent-first by last-active (falling back to created) time.
/// Shared by every scan entry point so the file and cache paths order identically.
pub(crate) fn sort_by_recent(sessions: &mut [SessionMeta]) {
    sessions.sort_by(|a, b| {
        let a_ts = a.last_active_at.or(a.created_at).unwrap_or(0);
        let b_ts = b.last_active_at.or(b.created_at).unwrap_or(0);
        b_ts.cmp(&a_ts)
    });
}

/// The file-parse-backed providers, in the same order the full scan fans them out.
/// SQLite-only sources (opencode.db / hermes state.db) are queried inside their
/// provider module and are intentionally not covered by the file cache.
/// pub(crate)：TUI worker 逐 provider 扫描并渐进回传时复用同一顺序。
pub(crate) const CACHED_PROVIDERS: [&str; 6] = [
    "codex", "claude", "opencode", "openclaw", "gemini", "hermes",
];

/// One logical Sessions page plus a look-ahead row. The stale first paint is
/// deliberately bounded by this value; the background revalidation still
/// produces the complete authoritative list.
pub(crate) const SCAN_CACHE_FIRST_PAINT_LIMIT: usize = 101;

/// Cache-aware scan of a single provider (sorted like `scan_sessions_for_provider`).
/// The "all providers" view iterates [`CACHED_PROVIDERS`] with this function in
/// the TUI session worker, emitting a progressive partial after each provider.
#[allow(dead_code)]
pub fn scan_sessions_cached_for_provider(
    store: &ScanCacheStore,
    provider_id: &str,
    force: bool,
) -> Vec<SessionMeta> {
    let mut sessions = provider_scan_cached(store, provider_id, force);
    sort_by_recent(&mut sessions);
    sessions
}

/// Scan one provider while streaming each available session back to the
/// detached TUI worker. File-backed providers invoke the callback during cache
/// reconciliation/parsing, before their complete Vec exists. Hybrid SQLite
/// providers keep their query on the same worker and stream its rows before the
/// final authoritative merge/sort.
pub(crate) fn scan_sessions_progressive_for_provider(
    store: Option<&ScanCacheStore>,
    provider_id: &str,
    force: bool,
    on_session: &mut dyn FnMut(&SessionMeta),
) -> Vec<SessionMeta> {
    match provider_id {
        "codex" => codex::scan_sessions_progressive(store, force, on_session),
        "claude" => claude::scan_sessions_progressive(store, force, on_session),
        "opencode" => opencode::scan_sessions_progressive(store, force, on_session),
        "openclaw" => openclaw::scan_sessions_progressive(store, force, on_session),
        "gemini" => gemini::scan_sessions_progressive(store, force, on_session),
        "hermes" => hermes::scan_sessions_progressive(store, force, on_session),
        _ => Vec::new(),
    }
}

/// Cancellation-aware provider scan for deep search. A cancelled scan returns
/// `None`; its partial rows are never treated as a complete search corpus.
pub(crate) fn scan_sessions_progressive_for_provider_cancellable(
    store: Option<&ScanCacheStore>,
    provider_id: &str,
    force: bool,
    on_session: &mut dyn FnMut(&SessionMeta),
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionMeta>> {
    match provider_id {
        "codex" => {
            codex::scan_sessions_progressive_cancellable(store, force, on_session, is_cancelled)
        }
        "claude" => {
            claude::scan_sessions_progressive_cancellable(store, force, on_session, is_cancelled)
        }
        "opencode" => {
            opencode::scan_sessions_progressive_cancellable(store, force, on_session, is_cancelled)
        }
        "openclaw" => {
            openclaw::scan_sessions_progressive_cancellable(store, force, on_session, is_cancelled)
        }
        "gemini" => {
            gemini::scan_sessions_progressive_cancellable(store, force, on_session, is_cancelled)
        }
        "hermes" => {
            hermes::scan_sessions_progressive_cancellable(store, force, on_session, is_cancelled)
        }
        _ => Some(Vec::new()),
    }
}

/// Bounded, owned-row provider stream for page-manifest construction. Unlike
/// the compatibility scan APIs above, this path never returns a provider-sized
/// `Vec`, never collects every target before parsing, and never loads every
/// sidecar row into a `HashMap`. The optional identity enricher is registered
/// only after base rows have reached the provisional sink; it applies bounded
/// provider metadata while the manifest consumes its identity-ordered run.
pub(crate) fn stream_sessions_for_provider_cancellable(
    store: Option<&ScanCacheStore>,
    provider_id: &str,
    force: bool,
    on_session: &mut dyn FnMut(SessionMeta) -> ControlFlow<()>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<
    (
        cache::StreamScanStats,
        Option<Box<dyn paged_manifest::IdentityRowEnricher>>,
    ),
    cache::StreamScanStop,
> {
    match provider_id {
        "codex" => codex::stream_sessions_cancellable(store, force, on_session, is_cancelled),
        "claude" => claude::stream_sessions_cancellable(store, force, on_session, is_cancelled)
            .map(|stats| (stats, None)),
        "opencode" => opencode::stream_sessions_cancellable(store, force, on_session, is_cancelled)
            .map(|stats| (stats, None)),
        "openclaw" => openclaw::stream_sessions_cancellable(store, force, on_session, is_cancelled)
            .map(|stats| (stats, None)),
        "gemini" => gemini::stream_sessions_cancellable(store, force, on_session, is_cancelled)
            .map(|stats| (stats, None)),
        "hermes" => hermes::stream_sessions_cancellable(store, force, on_session, is_cancelled)
            .map(|stats| (stats, None)),
        _ => Ok((cache::StreamScanStats::default(), None)),
    }
}

/// Build a fresh immutable page manifest for CLI session commands using the
/// bounded provider streams.
///
/// CLI commands are one-shot and cannot safely show a stale generation while a
/// background refresh catches up. Revalidating here preserves their previous
/// live-source semantics; the sidecar cache avoids reparsing unchanged files,
/// and external spill/merge keeps memory bounded. Each call publishes into a
/// leased, unique CLI namespace. It cannot advance the TUI cache's epoch or
/// cancel another CLI command scanning the same scope. TUI opening remains the
/// stale-while-revalidate, fixed-cost path.
pub(crate) fn build_fresh_session_manifest(
    scope: &str,
) -> Result<paged_manifest::ManifestReader, String> {
    let manifest_store = paged_manifest::CliManifestStore::open()
        .map_err(|error| format!("failed to open isolated session page workspace: {error}"))?;

    let scan_store = match ScanCacheStore::open() {
        Ok(store) => Some(store),
        Err(error) => {
            log::debug!(
                "[SESSION-CLI] scan sidecar unavailable; using authoritative source scan: {error}"
            );
            None
        }
    };
    let mut builder = manifest_store
        .begin_build(scope)
        .map_err(|error| format!("failed to start isolated session page build: {error}"))?;
    let one_provider = scope;
    let providers: &[&str] = if scope == "all" {
        &CACHED_PROVIDERS
    } else {
        std::slice::from_ref(&one_provider)
    };

    for provider_id in providers {
        let mut sink_error = None;
        let mut on_session = |row: SessionMeta| match builder.push(row) {
            Ok(()) => ControlFlow::Continue(()),
            Err(error) => {
                sink_error = Some(error.to_string());
                ControlFlow::Break(())
            }
        };
        let outcome = stream_sessions_for_provider_cancellable(
            scan_store.as_ref(),
            provider_id,
            false,
            &mut on_session,
            &|| false,
        );
        if let Some(error) = sink_error {
            return Err(format!("session page sink failed: {error}"));
        }
        match outcome {
            Ok((_stats, Some(enricher))) => builder
                .add_identity_enricher(enricher)
                .map_err(|error| format!("session title enrichment setup failed: {error}"))?,
            Ok((_stats, None)) => {}
            Err(reason) => {
                return Err(format!(
                    "session source scan for {provider_id} stopped before completion: {reason:?}"
                ));
            }
        }
    }

    builder
        .publish()
        .map(|published| published.reader)
        .map_err(|error| format!("failed to publish isolated session page snapshot: {error}"))
}

fn provider_scan_cached(
    store: &ScanCacheStore,
    provider_id: &str,
    force: bool,
) -> Vec<SessionMeta> {
    match provider_id {
        "codex" => codex::scan_sessions_cached(store, force),
        "claude" => claude::scan_sessions_cached(store, force),
        "opencode" => opencode::scan_sessions_cached(store, force),
        "openclaw" => openclaw::scan_sessions_cached(store, force),
        "gemini" => gemini::scan_sessions_cached(store, force),
        "hermes" => hermes::scan_sessions_cached(store, force),
        _ => Vec::new(),
    }
}

/// Build the stale-while-revalidate first paint from the bounded, schema-free
/// recency snapshot written by the previous authoritative scan. This never
/// queries or sorts the full SQLite sidecar table.
pub fn load_scan_cache_snapshot(provider_id: &str) -> Vec<SessionMeta> {
    recent_snapshot::load(provider_id)
}

/// A single search hit (matched message snippet inside a session).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSnippet {
    pub role: String,
    pub snippet: String,
}

/// Result of searching a session's full conversation content.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchHit {
    pub provider_id: String,
    pub session_id: String,
    pub source_path: String,
    pub snippets: Vec<SearchSnippet>,
}

/// Search the full conversation content of every session (across all providers)
/// for the given query. Returns one `SessionSearchHit` per session that contains
/// at least one match. This is a live, full-scan search — no index, always fresh.
#[allow(dead_code)]
pub fn search_sessions(query: &str) -> Vec<SessionSearchHit> {
    let sessions = scan_sessions();
    search_sessions_in(&sessions, query)
}

/// Same as `search_sessions`, but operates on an already-loaded session list.
pub fn search_sessions_in(sessions: &[SessionMeta], query: &str) -> Vec<SessionSearchHit> {
    search_sessions_in_cancellable(sessions, query, &|| false).unwrap_or_default()
}

/// Cancellation-aware full-content search. Provider tasks share the same
/// predicate and stop between files/messages; `None` means the complete result
/// was invalidated and must not be rendered.
pub(crate) fn search_sessions_in_cancellable(
    sessions: &[SessionMeta],
    query: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionSearchHit>> {
    let needle = query.trim();
    if needle.is_empty() {
        return Some(Vec::new());
    }

    let mut buckets: std::collections::HashMap<&str, Vec<&SessionMeta>> =
        std::collections::HashMap::new();
    for s in sessions {
        if is_cancelled() {
            return None;
        }
        buckets.entry(&s.provider_id).or_default().push(s);
    }

    let results: Option<Vec<Vec<SessionSearchHit>>> = std::thread::scope(|s| {
        let handles: Vec<_> = buckets
            .iter()
            .map(|(provider_id, metas)| {
                s.spawn(move || {
                    search_provider_cancellable(provider_id, metas, needle, is_cancelled)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().ok().flatten())
            .collect()
    });
    let results = results?;
    if is_cancelled() {
        return None;
    }

    let mut flat = Vec::new();
    for provider_hits in results {
        for hit in provider_hits {
            if is_cancelled() {
                return None;
            }
            flat.push(hit);
        }
    }
    flat.sort_by(|a, b| {
        if is_cancelled() {
            std::cmp::Ordering::Equal
        } else {
            a.provider_id
                .cmp(&b.provider_id)
                .then_with(|| a.session_id.cmp(&b.session_id))
        }
    });
    if is_cancelled() {
        None
    } else {
        Some(flat)
    }
}

fn search_provider_cancellable(
    provider_id: &str,
    metas: &[&SessionMeta],
    needle: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionSearchHit>> {
    match provider_id {
        "codex" => search_file_provider(metas, is_cancelled, |meta| {
            codex::search_session_cancellable(meta, needle, is_cancelled)
        }),
        "claude" => search_file_provider(metas, is_cancelled, |meta| {
            claude::search_session_cancellable(meta, needle, is_cancelled)
        }),
        "opencode" => opencode::search_sessions_cancellable(metas, needle, is_cancelled),
        "openclaw" => search_file_provider(metas, is_cancelled, |meta| {
            openclaw::search_session_cancellable(meta, needle, is_cancelled)
        }),
        "gemini" => search_file_provider(metas, is_cancelled, |meta| {
            gemini::search_session_cancellable(meta, needle, is_cancelled)
        }),
        "hermes" => hermes::search_sessions_cancellable(metas, needle, is_cancelled),
        _ => Some(Vec::new()),
    }
}

fn search_file_provider<F>(
    metas: &[&SessionMeta],
    is_cancelled: &(dyn Fn() -> bool + Sync),
    mut search: F,
) -> Option<Vec<SessionSearchHit>>
where
    F: FnMut(&SessionMeta) -> Option<SessionSearchHit>,
{
    let mut hits = Vec::new();
    for meta in metas {
        if is_cancelled() {
            return None;
        }
        if let Some(hit) = search(meta) {
            hits.push(hit);
        }
    }
    if is_cancelled() {
        None
    } else {
        Some(hits)
    }
}

pub fn load_messages(provider_id: &str, source_path: &str) -> Result<SessionMessageBatch, String> {
    load_messages_cancellable(provider_id, source_path, &|| false)
}

pub(crate) fn load_messages_cancellable(
    provider_id: &str,
    source_path: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<SessionMessageBatch, String> {
    // SQLite sessions use a "sqlite:" prefixed source_path
    if provider_id == "opencode" && source_path.starts_with("sqlite:") {
        return opencode::load_messages_sqlite_cancellable(source_path, is_cancelled);
    }
    if provider_id == "hermes" && source_path.starts_with("sqlite:") {
        return hermes::load_messages_sqlite_cancellable(source_path, is_cancelled);
    }

    let path = Path::new(source_path);
    match provider_id {
        "codex" => codex::load_messages_cancellable(path, is_cancelled),
        "claude" => claude::load_messages_cancellable(path, is_cancelled),
        "opencode" => opencode::load_messages_cancellable(path, is_cancelled),
        "openclaw" => openclaw::load_messages_cancellable(path, is_cancelled),
        "gemini" => gemini::load_messages_cancellable(path, is_cancelled),
        "hermes" => hermes::load_messages_cancellable(path, is_cancelled),
        _ => Err(format!("Unsupported provider: {provider_id}")),
    }
}

pub fn delete_session(
    provider_id: &str,
    session_id: &str,
    source_path: &str,
) -> Result<bool, String> {
    // SQLite sessions bypass the file-based deletion path
    if provider_id == "opencode" && source_path.starts_with("sqlite:") {
        return opencode::delete_session_sqlite(session_id, source_path);
    }
    if provider_id == "hermes" && source_path.starts_with("sqlite:") {
        return hermes::delete_session_sqlite(session_id, source_path);
    }

    let root = provider_root(provider_id)?;
    delete_session_with_root(provider_id, session_id, Path::new(source_path), &root)
}

/// Remove a successfully deleted session from every schema-free local cache.
/// Interactive callers adopt both typed sets of generations; CLI callers may
/// ignore the return value because the next process opens the updated pointers.
#[derive(Debug, Default)]
pub(crate) struct PurgedSessionManifests {
    pub(crate) base: Vec<(String, paged_manifest::PublishedManifest)>,
}

pub(crate) fn purge_deleted_session_from_local_caches(
    provider_id: &str,
    session_id: &str,
    source_path: &str,
) -> PurgedSessionManifests {
    scan_cache_store::purge_session(provider_id, session_id, source_path);

    let mut purged = PurgedSessionManifests::default();
    if let Ok(store) = paged_manifest::PagedManifestStore::open() {
        for scope in [provider_id, "all"] {
            match store.purge_identity(scope, provider_id, session_id, source_path) {
                Ok(Some(manifest)) => purged.base.push((scope.to_string(), manifest)),
                Ok(None) => {}
                Err(error) => {
                    log::debug!(
                        "[SESSION-PAGES] failed to purge deleted session from {scope}: {error}"
                    );
                }
            }
        }
    }
    purged
}

#[allow(dead_code)]
pub fn delete_sessions(requests: &[DeleteSessionRequest]) -> Vec<DeleteSessionOutcome> {
    collect_delete_session_outcomes(requests, |request| {
        delete_session(
            &request.provider_id,
            &request.session_id,
            &request.source_path,
        )
    })
}

fn delete_session_with_root(
    provider_id: &str,
    session_id: &str,
    source_path: &Path,
    root: &Path,
) -> Result<bool, String> {
    let validated_root = canonicalize_existing_path(root, "session root")?;
    let validated_source = canonicalize_existing_path(source_path, "session source")?;

    if !validated_source.starts_with(&validated_root) {
        return Err(format!(
            "Session source path is outside provider root: {}",
            source_path.display()
        ));
    }

    match provider_id {
        "codex" => codex::delete_session(&validated_root, &validated_source, session_id),
        "claude" => claude::delete_session(&validated_root, &validated_source, session_id),
        "opencode" => opencode::delete_session(&validated_root, &validated_source, session_id),
        "openclaw" => openclaw::delete_session(&validated_root, &validated_source, session_id),
        "gemini" => gemini::delete_session(&validated_root, &validated_source, session_id),
        "hermes" => hermes::delete_session(&validated_root, &validated_source, session_id),
        _ => Err(format!("Unsupported provider: {provider_id}")),
    }
}

fn provider_root(provider_id: &str) -> Result<PathBuf, String> {
    let root = match provider_id {
        "codex" => crate::codex_config::get_codex_config_dir().join("sessions"),
        "claude" => crate::config::get_claude_config_dir().join("projects"),
        "opencode" => opencode::get_opencode_data_dir(),
        "openclaw" => crate::openclaw_config::get_openclaw_dir().join("agents"),
        "gemini" => crate::gemini_config::get_gemini_dir().join("tmp"),
        "hermes" => crate::hermes_config::get_hermes_dir().join("sessions"),
        _ => return Err(format!("Unsupported provider: {provider_id}")),
    };

    Ok(root)
}

fn canonicalize_existing_path(path: &Path, label: &str) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!("{label} not found: {}", path.display()));
    }

    path.canonicalize()
        .map_err(|e| format!("Failed to resolve {label} {}: {e}", path.display()))
}

#[allow(dead_code)]
fn collect_delete_session_outcomes<F>(
    requests: &[DeleteSessionRequest],
    mut deleter: F,
) -> Vec<DeleteSessionOutcome>
where
    F: FnMut(&DeleteSessionRequest) -> Result<bool, String>,
{
    requests
        .iter()
        .map(|request| match deleter(request) {
            Ok(true) => DeleteSessionOutcome {
                provider_id: request.provider_id.clone(),
                session_id: request.session_id.clone(),
                source_path: request.source_path.clone(),
                success: true,
                error: None,
            },
            Ok(false) => DeleteSessionOutcome {
                provider_id: request.provider_id.clone(),
                session_id: request.session_id.clone(),
                source_path: request.source_path.clone(),
                success: false,
                error: Some("Session was not deleted".to_string()),
            },
            Err(error) => DeleteSessionOutcome {
                provider_id: request.provider_id.clone(),
                session_id: request.session_id.clone(),
                source_path: request.source_path.clone(),
                success: false,
                error: Some(error),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    #[test]
    fn message_batch_truncates_each_body_on_a_utf8_boundary() {
        let mut builder = SessionMessageBatchBuilder::new();
        let _ = builder.push(SessionMessage {
            role: "user".to_string(),
            content: "界".repeat(6_000),
            ts: None,
        });

        let batch = builder.finish();
        assert_eq!(batch.messages.len(), 1);
        assert!(batch.truncated);
        assert!(batch.messages[0].content.len() <= SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES);
        assert!(batch.messages[0]
            .content
            .is_char_boundary(batch.messages[0].content.len()));
    }

    #[test]
    fn message_batch_enforces_count_and_total_body_limits_before_retaining_more() {
        let mut bytes_builder = SessionMessageBatchBuilder::new();
        for _ in 0..SESSION_MESSAGE_PREVIEW_MAX_MESSAGES {
            if bytes_builder
                .push(SessionMessage {
                    role: "assistant".to_string(),
                    content: "x".repeat(SESSION_MESSAGE_PREVIEW_MAX_MESSAGE_BYTES),
                    ts: None,
                })
                .is_break()
            {
                break;
            }
        }
        let byte_batch = bytes_builder.finish();
        assert!(byte_batch.truncated);
        assert!(
            byte_batch
                .messages
                .iter()
                .map(|message| message.content.len())
                .sum::<usize>()
                <= SESSION_MESSAGE_PREVIEW_MAX_BODY_BYTES
        );

        let mut count_builder = SessionMessageBatchBuilder::new();
        for _ in 0..SESSION_MESSAGE_PREVIEW_MAX_MESSAGES + 1 {
            if count_builder
                .push(SessionMessage {
                    role: "user".to_string(),
                    content: String::new(),
                    ts: None,
                })
                .is_break()
            {
                break;
            }
        }
        let count_batch = count_builder.finish();
        assert_eq!(
            count_batch.messages.len(),
            SESSION_MESSAGE_PREVIEW_MAX_MESSAGES
        );
        assert!(count_batch.truncated);
    }

    #[test]
    fn deep_search_cancellation_propagates_into_provider_message_loop() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("session.jsonl");
        let mut transcript = String::new();
        for index in 0..2_000 {
            transcript.push_str(&format!(
                "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"line {index}\"}}}}\n"
            ));
        }
        std::fs::write(&source, transcript).expect("write transcript");
        let sessions = vec![SessionMeta {
            provider_id: "codex".to_string(),
            session_id: "session-1".to_string(),
            source_path: Some(source.to_string_lossy().to_string()),
            ..SessionMeta::default()
        }];
        let checks = AtomicUsize::new(0);
        let is_cancelled = || checks.fetch_add(1, Ordering::AcqRel) >= 12;

        let result = search_sessions_in_cancellable(&sessions, "never-present", &is_cancelled);

        assert!(
            result.is_none(),
            "cancelled provider work must not look complete"
        );
        assert!(
            checks.load(Ordering::Acquire) < 100,
            "provider loop should observe cancellation near the requested line"
        );
    }

    #[test]
    fn rejects_source_path_outside_provider_root() {
        let root = tempdir().expect("tempdir");
        let outside = tempdir().expect("tempdir");
        let source = outside.path().join("session.jsonl");
        std::fs::write(&source, "{}").expect("write source");

        let err = delete_session_with_root("codex", "session-1", &source, root.path())
            .expect_err("expected outside-root path to be rejected");

        assert!(err.contains("outside provider root"));
    }

    #[test]
    fn rejects_missing_source_path() {
        let root = tempdir().expect("tempdir");
        let missing = root.path().join("missing.jsonl");

        let err = delete_session_with_root("codex", "session-1", &missing, root.path())
            .expect_err("expected missing source path to fail");

        assert!(err.contains("session source not found"));
    }

    #[test]
    fn batch_delete_collects_successes_and_failures_in_order() {
        let requests = vec![
            DeleteSessionRequest {
                provider_id: "codex".to_string(),
                session_id: "s1".to_string(),
                source_path: "/tmp/s1".to_string(),
            },
            DeleteSessionRequest {
                provider_id: "claude".to_string(),
                session_id: "s2".to_string(),
                source_path: "/tmp/s2".to_string(),
            },
            DeleteSessionRequest {
                provider_id: "gemini".to_string(),
                session_id: "s3".to_string(),
                source_path: "/tmp/s3".to_string(),
            },
        ];

        let outcomes = collect_delete_session_outcomes(&requests, |request| {
            match request.session_id.as_str() {
                "s1" => Ok(true),
                "s2" => Err("boom".to_string()),
                _ => Ok(false),
            }
        });

        assert_eq!(outcomes.len(), 3);
        assert!(outcomes[0].success);
        assert_eq!(outcomes[0].error, None);
        assert!(!outcomes[1].success);
        assert_eq!(outcomes[1].error.as_deref(), Some("boom"));
        assert!(!outcomes[2].success);
        assert_eq!(
            outcomes[2].error.as_deref(),
            Some("Session was not deleted")
        );
    }
}
