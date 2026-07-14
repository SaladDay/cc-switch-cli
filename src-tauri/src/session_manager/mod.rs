pub mod cache;
pub mod providers;
pub mod scan_cache_store;
pub mod terminal;

use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<i64>,
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
    let mut sessions = match provider_id {
        "codex" => codex::scan_sessions(),
        "claude" => claude::scan_sessions(),
        "opencode" => opencode::scan_sessions(),
        "openclaw" => openclaw::scan_sessions(),
        "gemini" => gemini::scan_sessions(),
        "hermes" => hermes::scan_sessions(),
        _ => Vec::new(),
    };

    sort_by_recent(&mut sessions);
    sessions
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

/// Cache-aware scan of a single provider (sorted like `scan_sessions_for_provider`).
/// The "all providers" view iterates [`CACHED_PROVIDERS`] with this function in
/// the TUI session worker, emitting a progressive partial after each provider.
pub fn scan_sessions_cached_for_provider(
    store: &ScanCacheStore,
    provider_id: &str,
    force: bool,
) -> Vec<SessionMeta> {
    let mut sessions = provider_scan_cached(store, provider_id, force);
    sort_by_recent(&mut sessions);
    sessions
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

/// Build the stale-while-revalidate first paint entirely from the persistent
/// cache: no directory walk, no file reads, just the JSON already stored. Returns
/// an empty list on the first-ever run (empty cache) so the caller falls straight
/// through to the full scan. `provider_id == "all"` merges every provider's cache.
pub fn load_scan_cache_snapshot(store: &ScanCacheStore, provider_id: &str) -> Vec<SessionMeta> {
    let provider = (provider_id != "all").then_some(provider_id);
    let meta_jsons = store
        .load_meta_json(provider, cache::SCAN_CACHE_VERSION)
        .unwrap_or_default();
    let mut sessions: Vec<SessionMeta> = meta_jsons
        .iter()
        .filter_map(|json| serde_json::from_str(json).ok())
        .collect();
    sort_by_recent(&mut sessions);
    sessions
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
    let needle = query.trim();
    if needle.is_empty() {
        return Vec::new();
    }

    let mut buckets: std::collections::HashMap<&str, Vec<&SessionMeta>> =
        std::collections::HashMap::new();
    for s in sessions {
        buckets.entry(&s.provider_id).or_default().push(s);
    }

    let results: Vec<Vec<SessionSearchHit>> = std::thread::scope(|s| {
        let handles: Vec<_> = buckets
            .iter()
            .map(|(provider_id, metas)| {
                s.spawn(move || search_provider(provider_id, metas, needle))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });

    let mut flat: Vec<SessionSearchHit> = results.into_iter().flatten().collect();
    flat.sort_by(|a, b| {
        a.provider_id
            .cmp(&b.provider_id)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    flat
}

fn search_provider(
    provider_id: &str,
    metas: &[&SessionMeta],
    needle: &str,
) -> Vec<SessionSearchHit> {
    match provider_id {
        "codex" => metas
            .iter()
            .filter_map(|m| codex::search_session(m, needle))
            .collect(),
        "claude" => metas
            .iter()
            .filter_map(|m| claude::search_session(m, needle))
            .collect(),
        "opencode" => opencode::search_sessions(metas, needle),
        "openclaw" => metas
            .iter()
            .filter_map(|m| openclaw::search_session(m, needle))
            .collect(),
        "gemini" => metas
            .iter()
            .filter_map(|m| gemini::search_session(m, needle))
            .collect(),
        "hermes" => hermes::search_sessions(metas, needle),
        _ => Vec::new(),
    }
}

pub fn load_messages(provider_id: &str, source_path: &str) -> Result<Vec<SessionMessage>, String> {
    // SQLite sessions use a "sqlite:" prefixed source_path
    if provider_id == "opencode" && source_path.starts_with("sqlite:") {
        return opencode::load_messages_sqlite(source_path);
    }
    if provider_id == "hermes" && source_path.starts_with("sqlite:") {
        return hermes::load_messages_sqlite(source_path);
    }

    let path = Path::new(source_path);
    match provider_id {
        "codex" => codex::load_messages(path),
        "claude" => claude::load_messages(path),
        "opencode" => opencode::load_messages(path),
        "openclaw" => openclaw::load_messages(path),
        "gemini" => gemini::load_messages(path),
        "hermes" => hermes::load_messages(path),
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
    use tempfile::tempdir;

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
