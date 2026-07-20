use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufReader, Read};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::de::{DeserializeSeed, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::Value;

use crate::openclaw_config::get_openclaw_dir;
use crate::session_manager::cache::{self, FileScanTarget};
use crate::session_manager::scan_cache_store::ScanCacheStore;
use crate::{
    config::write_json_file,
    session_manager::{
        SearchSnippet, SessionMessage, SessionMessageBatch, SessionMessageBatchBuilder,
        SessionMeta, SessionSearchHit,
    },
};

use super::utils::{
    build_snippet_cancellable, extract_text, parse_timestamp_to_ms, path_basename,
    read_head_tail_lines_bounded, truncate_summary, visit_bounded_lines_cancellable,
    visit_bounded_lines_cancellable_with_status, TITLE_MAX_CHARS,
};

const PROVIDER_ID: &str = "openclaw";

// `sessions.json` is optional decoration, so it must never dominate a session
// scan. A scanner retains at most one small index per parser worker, and each
// index is bounded both by encoded bytes and by logical entries. Oversized or
// malformed indexes deliberately degrade to the title parsed from the JSONL.
const DISPLAY_NAME_CACHE_DIRS: usize = 4;
const DISPLAY_NAME_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const DISPLAY_NAME_MAX_ENTRIES: usize = 4_096;

/// Strip trailing `\n[message_id: ...]` metadata injected by OpenClaw gateway.
fn strip_message_id_suffix(text: &str) -> &str {
    if let Some(pos) = text.rfind("\n[message_id:") {
        text[..pos].trim_end()
    } else {
        text
    }
}

pub fn scan_sessions() -> Vec<SessionMeta> {
    let agents_dir = get_openclaw_dir().join("agents");
    if !agents_dir.exists() {
        return Vec::new();
    }

    let mut sessions = Vec::new();
    let display_names = DisplayNameIndexCache::new();

    // Traverse each agent directory
    let agent_entries = match std::fs::read_dir(&agents_dir) {
        Ok(entries) => entries,
        Err(_) => return sessions,
    };

    for agent_entry in agent_entries.flatten() {
        let agent_path = agent_entry.path();
        if !agent_path.is_dir() {
            continue;
        }

        let sessions_dir = agent_path.join("sessions");
        if !sessions_dir.is_dir() {
            continue;
        }

        let session_entries = match std::fs::read_dir(&sessions_dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        let names = display_names
            .get(&sessions_dir, &|| false)
            .unwrap_or_else(empty_display_names);

        for entry in session_entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }

            if let Some(meta) = parse_session(&path, Some(names.as_ref())) {
                sessions.push(meta);
            }
        }
    }

    sessions
}

/// Cache-aware scan over `agents/<agent>/sessions/*.jsonl`.
pub(crate) fn scan_sessions_cached(store: &ScanCacheStore, force: bool) -> Vec<SessionMeta> {
    let display_names = DisplayNameIndexCache::new();
    cache::scan_provider_cached(
        store,
        PROVIDER_ID,
        scan_targets(),
        force,
        |path| parse_meta(path, &display_names, &|| false),
        |_| true,
    )
}

pub(crate) fn scan_sessions_progressive(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(&SessionMeta),
) -> Vec<SessionMeta> {
    let targets = scan_targets();
    let display_names = DisplayNameIndexCache::new();
    match store {
        Some(store) => cache::scan_provider_cached_progressive(
            store,
            PROVIDER_ID,
            targets,
            force,
            |path| parse_meta(path, &display_names, &|| false),
            |_| true,
            on_session,
        ),
        None => cache::scan_provider_uncached_progressive(
            targets,
            |path| parse_meta(path, &display_names, &|| false),
            on_session,
        ),
    }
}

pub(crate) fn scan_sessions_progressive_cancellable(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(&SessionMeta),
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionMeta>> {
    let targets = scan_targets_in_cancellable(&get_openclaw_dir().join("agents"), is_cancelled)?;
    let display_names = DisplayNameIndexCache::new();
    match store {
        Some(store) => cache::scan_provider_cached_progressive_cancellable(
            store,
            PROVIDER_ID,
            targets,
            force,
            |path| parse_meta(path, &display_names, is_cancelled),
            |_| true,
            on_session,
            is_cancelled,
        ),
        None => cache::scan_provider_uncached_progressive_cancellable(
            targets,
            |path| parse_meta(path, &display_names, is_cancelled),
            on_session,
            is_cancelled,
        ),
    }
}

pub(crate) fn stream_sessions_cancellable(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(SessionMeta) -> ControlFlow<()>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<cache::StreamScanStats, cache::StreamScanStop> {
    let agents_dir = get_openclaw_dir().join("agents");
    let display_names = DisplayNameIndexCache::new();
    cache::stream_file_provider_cancellable(
        store,
        PROVIDER_ID,
        force,
        |path| parse_meta_authoritative(path, &display_names, is_cancelled),
        |_| true,
        fingerprint_target,
        move |on_target, cancel| visit_stream_targets(&agents_dir, on_target, cancel),
        on_session,
        is_cancelled,
    )
}

fn visit_stream_targets(
    agents_dir: &Path,
    on_target: &mut dyn FnMut(FileScanTarget) -> Result<(), cache::StreamScanStop>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), cache::StreamScanStop> {
    if is_cancelled() {
        return Err(cache::StreamScanStop::Cancelled);
    }
    let agents = match std::fs::read_dir(agents_dir) {
        Ok(agents) => agents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            log::warn!(
                "authoritative OpenClaw agent walk failed at {}: {error}",
                agents_dir.display()
            );
            return Err(cache::StreamScanStop::Incomplete);
        }
    };
    for agent in agents {
        if is_cancelled() {
            return Err(cache::StreamScanStop::Cancelled);
        }
        let agent = agent.map_err(|error| {
            log::warn!(
                "authoritative OpenClaw agent entry failed at {}: {error}",
                agents_dir.display()
            );
            cache::StreamScanStop::Incomplete
        })?;
        let kind = agent.file_type().map_err(|error| {
            log::warn!(
                "authoritative OpenClaw agent type failed at {}: {error}",
                agent.path().display()
            );
            cache::StreamScanStop::Incomplete
        })?;
        if !kind.is_dir() {
            continue;
        }
        let sessions_dir = agent.path().join("sessions");
        let display_names = sessions_dir.join("sessions.json");
        let mut decorate = |mut target: FileScanTarget| {
            cache::mix_sibling_into_fingerprint_strict(&mut target, &display_names).map_err(
                |error| {
                    log::warn!(
                        "authoritative OpenClaw display-name fingerprint failed at {}: {error}",
                        display_names.display()
                    );
                    cache::StreamScanStop::Incomplete
                },
            )?;
            on_target(target)
        };
        cache::visit_targets_flat_cancellable(&sessions_dir, "jsonl", &mut decorate, is_cancelled)?;
    }
    Ok(())
}

fn fingerprint_target(path: &Path) -> Option<FileScanTarget> {
    let mut target = cache::stat_target_strict(path).ok().flatten()?;
    cache::mix_sibling_into_fingerprint_strict(&mut target, &path.parent()?.join("sessions.json"))
        .ok()?;
    Some(target)
}

fn scan_targets() -> Vec<FileScanTarget> {
    scan_targets_in(&get_openclaw_dir().join("agents"))
}

/// 收集 `agents/<agent>/sessions/*.jsonl` 的直属文件。与 `scan_sessions` 一致，
/// 只扫 `sessions/` 单层目录（用平铺收集器而非递归），嵌套子目录里的文件不纳入
/// ——否则缓存路径会展示旧路径看不到的文件，且其旁路 `sessions.json` 定位会取错
/// 目录。
fn scan_targets_in(agents_dir: &Path) -> Vec<FileScanTarget> {
    scan_targets_in_cancellable(agents_dir, &|| false)
        .expect("non-cancellable target scan cannot stop")
}

fn scan_targets_in_cancellable(
    agents_dir: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<FileScanTarget>> {
    let mut targets = Vec::new();
    if is_cancelled() {
        return None;
    }
    let agent_entries = match std::fs::read_dir(agents_dir) {
        Ok(entries) => entries,
        Err(_) => return Some(targets),
    };
    for agent_entry in agent_entries.flatten() {
        if is_cancelled() {
            return None;
        }
        let sessions_dir = agent_entry.path().join("sessions");
        if !sessions_dir.is_dir() {
            continue;
        }
        let mut agent_targets = Vec::new();
        if !cache::collect_targets_flat_cancellable(
            &sessions_dir,
            "jsonl",
            &mut agent_targets,
            is_cancelled,
        ) {
            return None;
        }
        // 标题派生自旁路的 sessions.json 显示名索引：它变化时缓存也必须失效
        let display_names = sessions_dir.join("sessions.json");
        for target in &mut agent_targets {
            if is_cancelled() {
                return None;
            }
            cache::mix_sibling_into_fingerprint(target, &display_names);
        }
        targets.extend(agent_targets);
    }
    Some(targets)
}

/// Parse one session file with the scan-scoped, bounded display-name cache.
fn parse_meta(
    path: &Path,
    display_names: &DisplayNameIndexCache,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<SessionMeta> {
    if is_cancelled() {
        return None;
    }
    let names = path
        .parent()
        .and_then(|sessions_dir| display_names.get(sessions_dir, is_cancelled))?;
    let meta = parse_session(path, Some(names.as_ref()))?;
    (!is_cancelled()).then_some(meta)
}

fn parse_meta_authoritative(
    path: &Path,
    display_names: &DisplayNameIndexCache,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<SessionMeta>, cache::StreamScanStop> {
    if is_cancelled() {
        return Err(cache::StreamScanStop::Cancelled);
    }
    let Some(sessions_dir) = path.parent() else {
        return Err(cache::StreamScanStop::Incomplete);
    };
    let names = match display_names.get(sessions_dir, is_cancelled) {
        Some(names) => names,
        None if is_cancelled() => return Err(cache::StreamScanStop::Cancelled),
        None => return Err(cache::StreamScanStop::Incomplete),
    };
    let meta = parse_session_authoritative(path, Some(names.as_ref()))?;
    if is_cancelled() {
        Err(cache::StreamScanStop::Cancelled)
    } else {
        Ok(meta)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisplayNameEntry {
    session_id: Option<String>,
    display_name: Option<String>,
}

struct DisplayNameMapSeed<'a> {
    is_cancelled: &'a (dyn Fn() -> bool + Sync),
}

impl<'de> DeserializeSeed<'de> for DisplayNameMapSeed<'_> {
    type Value = HashMap<String, String>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(DisplayNameMapVisitor {
            is_cancelled: self.is_cancelled,
        })
    }
}

struct DisplayNameMapVisitor<'a> {
    is_cancelled: &'a (dyn Fn() -> bool + Sync),
}

impl<'de> Visitor<'de> for DisplayNameMapVisitor<'_> {
    type Value = HashMap<String, String>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an OpenClaw session display-name object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut names = HashMap::new();
        let mut entries = 0usize;
        loop {
            if (self.is_cancelled)() {
                return Err(serde::de::Error::custom(
                    "display-name index load cancelled",
                ));
            }
            let Some(_key) = map.next_key::<String>()? else {
                break;
            };
            entries = entries.saturating_add(1);
            if entries > DISPLAY_NAME_MAX_ENTRIES {
                return Err(serde::de::Error::custom(
                    "display-name index entry limit exceeded",
                ));
            }
            let entry = map.next_value::<DisplayNameEntry>()?;
            if let (Some(session_id), Some(display_name)) = (
                entry.session_id,
                entry.display_name.filter(|name| !name.is_empty()),
            ) {
                names.insert(session_id, truncate_summary(&display_name, TITLE_MAX_CHARS));
            }
        }
        Ok(names)
    }
}

enum DisplayNameLoad {
    Ready(HashMap<String, String>),
    Cancelled,
}

#[derive(Default)]
struct DisplayNameCacheState {
    indexes: VecDeque<(PathBuf, Arc<HashMap<String, String>>)>,
}

/// Fixed-capacity LRU shared by all parser workers in one provider scan.
///
/// Holding the mutex while the small index is first parsed guarantees that a
/// cold scan never opens the same `sessions.json` once per session. The parser
/// checks cancellation on every entry, so a waiter is released promptly too.
struct DisplayNameIndexCache {
    state: Mutex<DisplayNameCacheState>,
    index_file_reads: std::sync::atomic::AtomicUsize,
}

impl DisplayNameIndexCache {
    fn new() -> Self {
        Self {
            state: Mutex::new(DisplayNameCacheState::default()),
            index_file_reads: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn get(
        &self,
        sessions_dir: &Path,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Option<Arc<HashMap<String, String>>> {
        if is_cancelled() {
            return None;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if is_cancelled() {
            return None;
        }

        if let Some(index) = state
            .indexes
            .iter()
            .position(|(cached_dir, _)| cached_dir == sessions_dir)
        {
            if let Some((cached_dir, names)) = state.indexes.remove(index) {
                let result = Arc::clone(&names);
                state.indexes.push_back((cached_dir, names));
                return Some(result);
            }
        }

        let names = match self.load(sessions_dir, is_cancelled) {
            DisplayNameLoad::Ready(names) => Arc::new(names),
            DisplayNameLoad::Cancelled => return None,
        };
        if state.indexes.len() >= DISPLAY_NAME_CACHE_DIRS {
            state.indexes.pop_front();
        }
        state
            .indexes
            .push_back((sessions_dir.to_path_buf(), Arc::clone(&names)));
        Some(names)
    }

    fn load(
        &self,
        sessions_dir: &Path,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> DisplayNameLoad {
        if is_cancelled() {
            return DisplayNameLoad::Cancelled;
        }
        let index_path = sessions_dir.join("sessions.json");
        let metadata = match std::fs::metadata(&index_path) {
            Ok(metadata) if metadata.is_file() && metadata.len() <= DISPLAY_NAME_MAX_FILE_BYTES => {
                metadata
            }
            _ => return DisplayNameLoad::Ready(HashMap::new()),
        };
        debug_assert!(metadata.len() <= DISPLAY_NAME_MAX_FILE_BYTES);
        if is_cancelled() {
            return DisplayNameLoad::Cancelled;
        }
        let file = match File::open(&index_path) {
            Ok(file) => file,
            Err(_) => return DisplayNameLoad::Ready(HashMap::new()),
        };
        self.index_file_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Enforce the byte ceiling on the reader too: the file may grow after
        // the metadata check, and one enormous JSON string must remain bounded.
        let reader = BufReader::new(file).take(DISPLAY_NAME_MAX_FILE_BYTES + 1);
        let mut deserializer = serde_json::Deserializer::from_reader(reader);
        let parsed = DisplayNameMapSeed { is_cancelled }.deserialize(&mut deserializer);
        if is_cancelled() {
            return DisplayNameLoad::Cancelled;
        }
        match parsed {
            Ok(names) if deserializer.end().is_ok() => DisplayNameLoad::Ready(names),
            _ => DisplayNameLoad::Ready(HashMap::new()),
        }
    }

    #[cfg(test)]
    fn index_file_reads(&self) -> usize {
        self.index_file_reads
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    fn cached_dir_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .indexes
            .len()
    }
}

fn empty_display_names() -> Arc<HashMap<String, String>> {
    Arc::new(HashMap::new())
}

pub fn load_messages(path: &Path) -> Result<SessionMessageBatch, String> {
    load_messages_cancellable(path, &|| false)
}

pub(crate) fn load_messages_cancellable(
    path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<SessionMessageBatch, String> {
    let mut batch = SessionMessageBatchBuilder::new();
    let status = visit_bounded_lines_cancellable_with_status(path, is_cancelled, &mut |line| {
        let value: Value = match serde_json::from_str(line) {
            Ok(parsed) => parsed,
            Err(_) => return ControlFlow::Continue(()),
        };

        if value.get("type").and_then(Value::as_str) != Some("message") {
            return ControlFlow::Continue(());
        }

        let message = match value.get("message") {
            Some(msg) => msg,
            None => return ControlFlow::Continue(()),
        };

        let raw_role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        // Map OpenClaw roles to our standard roles
        let role = match raw_role {
            "toolResult" => "tool".to_string(),
            other => other.to_string(),
        };

        let content = message.get("content").map(extract_text).unwrap_or_default();
        if content.trim().is_empty() {
            return ControlFlow::Continue(());
        }

        let ts = value.get("timestamp").and_then(parse_timestamp_to_ms);

        batch.push(SessionMessage { role, content, ts })
    })
    .map_err(|error| format!("Failed to read session file: {error}"))?
    .ok_or_else(|| "Session message preview was cancelled".to_string())?;
    if status.oversized_record_skipped {
        batch.mark_truncated();
    }

    Ok(batch.finish())
}

/// Search a single OpenClaw session file for `needle` (case-insensitive).
#[allow(dead_code)]
pub fn search_session(meta: &SessionMeta, needle: &str) -> Option<SessionSearchHit> {
    search_session_cancellable(meta, needle, &|| false)
}

pub(crate) fn search_session_cancellable(
    meta: &SessionMeta,
    needle: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<SessionSearchHit> {
    if is_cancelled() {
        return None;
    }
    let source_path = meta.source_path.as_deref()?;
    let path = Path::new(source_path);
    let mut snippets: Vec<SearchSnippet> = Vec::new();
    const MAX_SNIPPETS: usize = 5;

    visit_bounded_lines_cancellable(path, is_cancelled, &mut |line| {
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => return ControlFlow::Continue(()),
        };
        if value.get("type").and_then(Value::as_str) != Some("message") {
            return ControlFlow::Continue(());
        }
        let message = match value.get("message") {
            Some(message) => message,
            None => return ControlFlow::Continue(()),
        };
        let raw_role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let role = match raw_role {
            "toolResult" => "tool".to_string(),
            other => other.to_string(),
        };
        let content = message.get("content").map(extract_text).unwrap_or_default();
        if content.trim().is_empty() {
            return ControlFlow::Continue(());
        }
        match build_snippet_cancellable(&content, needle, is_cancelled) {
            Ok(Some(snippet)) => {
                snippets.push(SearchSnippet { role, snippet });
                if snippets.len() >= MAX_SNIPPETS {
                    return ControlFlow::Break(());
                }
            }
            Ok(None) => {}
            Err(_) => return ControlFlow::Break(()),
        }
        ControlFlow::Continue(())
    })
    .ok()??;
    if is_cancelled() {
        return None;
    }
    if snippets.is_empty() {
        return None;
    }
    Some(SessionSearchHit {
        provider_id: PROVIDER_ID.to_string(),
        session_id: meta.session_id.clone(),
        source_path: source_path.to_string(),
        snippets,
    })
}

pub fn delete_session(_root: &Path, path: &Path, session_id: &str) -> Result<bool, String> {
    let meta = parse_session(path, None).ok_or_else(|| {
        format!(
            "Failed to parse OpenClaw session metadata: {}",
            path.display()
        )
    })?;

    if meta.session_id != session_id {
        return Err(format!(
            "OpenClaw session ID mismatch: expected {session_id}, found {}",
            meta.session_id
        ));
    }

    let index_path = path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join("sessions.json");
    prune_sessions_index(&index_path, session_id, path)?;

    std::fs::remove_file(path).map_err(|e| {
        format!(
            "Failed to delete OpenClaw session file {}: {e}",
            path.display()
        )
    })?;

    Ok(true)
}

fn parse_session(
    path: &Path,
    display_names: Option<&HashMap<String, String>>,
) -> Option<SessionMeta> {
    parse_session_authoritative(path, display_names)
        .ok()
        .flatten()
}

fn parse_session_authoritative(
    path: &Path,
    display_names: Option<&HashMap<String, String>>,
) -> Result<Option<SessionMeta>, cache::StreamScanStop> {
    let (head, tail) = read_head_tail_lines_bounded(path, 10, 30).map_err(|error| {
        log::warn!(
            "authoritative OpenClaw metadata read failed at {}: {error}",
            path.display()
        );
        cache::StreamScanStop::Incomplete
    })?;
    Ok(parse_session_lines(path, display_names, &head, &tail))
}

fn parse_session_lines(
    path: &Path,
    display_names: Option<&HashMap<String, String>>,
    head: &[String],
    tail: &[String],
) -> Option<SessionMeta> {
    let mut session_id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut created_at: Option<i64> = None;
    let mut summary: Option<String> = None;
    let mut first_user_message: Option<String> = None;

    // Extract metadata, summary, and first user message from head lines
    for line in head {
        let value: Value = match serde_json::from_str(line) {
            Ok(parsed) => parsed,
            Err(_) => continue,
        };

        if created_at.is_none() {
            created_at = value.get("timestamp").and_then(parse_timestamp_to_ms);
        }

        let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");

        if event_type == "session" {
            if session_id.is_none() {
                session_id = value
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string());
            }
            if cwd.is_none() {
                cwd = value
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string());
            }
            if let Some(ts) = value.get("timestamp").and_then(parse_timestamp_to_ms) {
                created_at.get_or_insert(ts);
            }
            continue;
        }

        if event_type == "message" {
            if let Some(message) = value.get("message") {
                let text = message.get("content").map(extract_text).unwrap_or_default();
                let cleaned = strip_message_id_suffix(&text);
                if !cleaned.trim().is_empty() {
                    if first_user_message.is_none()
                        && message.get("role").and_then(Value::as_str) == Some("user")
                    {
                        first_user_message = Some(cleaned.trim().to_string());
                    }
                    if summary.is_none() {
                        summary = Some(cleaned.trim().to_string());
                    }
                }
            }
        }

        if session_id.is_some()
            && cwd.is_some()
            && created_at.is_some()
            && summary.is_some()
            && first_user_message.is_some()
        {
            break;
        }
    }

    // Extract last_active_at from tail lines (reverse order)
    let mut last_active_at: Option<i64> = None;
    for line in tail.iter().rev() {
        let value: Value = match serde_json::from_str(line) {
            Ok(parsed) => parsed,
            Err(_) => continue,
        };
        if let Some(ts) = value.get("timestamp").and_then(parse_timestamp_to_ms) {
            last_active_at = Some(ts);
            break;
        }
    }

    // Fall back to filename as session ID
    let session_id = session_id.or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
    });
    let session_id = session_id?;

    // Title priority: displayName (from sessions.json) > first user message > dir basename
    let title = display_names
        .and_then(|m| m.get(&session_id))
        .filter(|s| !s.is_empty())
        .map(|t| truncate_summary(t, TITLE_MAX_CHARS))
        .or_else(|| first_user_message.map(|t| truncate_summary(&t, TITLE_MAX_CHARS)))
        .or_else(|| {
            cwd.as_deref()
                .and_then(path_basename)
                .map(|s| s.to_string())
        });

    let summary = summary.map(|text| truncate_summary(&text, 160));

    Some(SessionMeta {
        provider_id: PROVIDER_ID.to_string(),
        session_id: session_id.clone(),
        title,
        summary,
        project_dir: cwd,
        created_at,
        last_active_at,
        source_path: Some(path.to_string_lossy().to_string()),
        resume_command: None, // OpenClaw sessions are gateway-managed, no CLI resume
    })
}

fn prune_sessions_index(
    index_path: &Path,
    session_id: &str,
    source_path: &Path,
) -> Result<(), String> {
    if !index_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(index_path).map_err(|e| {
        format!(
            "Failed to read OpenClaw sessions index {}: {e}",
            index_path.display()
        )
    })?;
    let mut index: serde_json::Map<String, Value> =
        serde_json::from_str(&content).map_err(|e| {
            format!(
                "Failed to parse OpenClaw sessions index {}: {e}",
                index_path.display()
            )
        })?;

    let source = source_path.to_string_lossy();
    index.retain(|_, entry| {
        let same_id = entry.get("sessionId").and_then(Value::as_str) == Some(session_id);
        let same_file = entry.get("sessionFile").and_then(Value::as_str) == Some(source.as_ref());
        !(same_id || same_file)
    });

    write_json_file(index_path, &index).map_err(|e| {
        format!(
            "Failed to update OpenClaw sessions index {}: {e}",
            index_path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn scan_targets_ignores_nested_session_subdirectories() {
        let temp = tempdir().expect("tempdir");
        let agents_dir = temp.path();
        let sessions = agents_dir.join("main").join("sessions");
        std::fs::create_dir_all(sessions.join("archive")).expect("mkdir");
        // sessions/ 直属会话文件应被收集
        std::fs::write(sessions.join("session-1.jsonl"), "{}\n").expect("write");
        // 嵌套子目录里的会话文件不应被缓存扫描收集
        std::fs::write(sessions.join("archive").join("session-2.jsonl"), "{}\n").expect("write");

        let targets = scan_targets_in(agents_dir);
        assert_eq!(targets.len(), 1, "只收集 sessions/ 直属文件");
        assert!(targets
            .iter()
            .any(|t| t.path.file_name().and_then(|n| n.to_str()) == Some("session-1.jsonl")));
        assert!(!targets
            .iter()
            .any(|t| t.path.file_name().and_then(|n| n.to_str()) == Some("session-2.jsonl")));
    }

    #[test]
    fn parse_session_uses_first_user_message_as_title() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session-abc.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"session\",\"id\":\"session-abc\",\"cwd\":\"/tmp/project\",\"timestamp\":\"2026-03-06T10:00:00Z\"}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"How do I deploy?\"},\"timestamp\":\"2026-03-06T10:01:00Z\"}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":\"Here is how...\"},\"timestamp\":\"2026-03-06T10:02:00Z\"}\n"
            ),
        )
        .expect("write");

        let meta = parse_session(&path, None).unwrap();
        assert_eq!(meta.title.as_deref(), Some("How do I deploy?"));
    }

    #[test]
    fn parse_session_display_name_overrides_user_message() {
        let temp = tempdir().expect("tempdir");
        let sessions_dir = temp.path();

        let path = sessions_dir.join("session-abc.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"session\",\"id\":\"session-abc\",\"cwd\":\"/tmp/project\",\"timestamp\":\"2026-03-06T10:00:00Z\"}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"fix something\"},\"timestamp\":\"2026-03-06T10:01:00Z\"}\n"
            ),
        )
        .expect("write session");

        std::fs::write(
            sessions_dir.join("sessions.json"),
            r#"{
                "agent:main:main": {
                    "sessionId": "session-abc",
                    "displayName": "重构登录模块"
                }
            }"#,
        )
        .expect("write index");

        let display_names = DisplayNameIndexCache::new();
        let meta = parse_meta(&path, &display_names, &|| false).expect("parse metadata");
        assert_eq!(meta.title.as_deref(), Some("重构登录模块"));
        assert_eq!(display_names.index_file_reads(), 1);
    }

    #[test]
    fn one_agent_reads_display_name_index_once_for_many_sessions() {
        let temp = tempdir().expect("tempdir");
        let sessions_dir = temp.path();
        let mut index = serde_json::Map::new();
        let mut paths = Vec::new();
        for value in 0..128 {
            index.insert(
                format!("agent:{value}"),
                serde_json::json!({
                    "sessionId": format!("session-{value}"),
                    "displayName": format!("name-{value}")
                }),
            );
            let path = sessions_dir.join(format!("session-{value}.jsonl"));
            std::fs::write(
                &path,
                format!(
                    "{{\"type\":\"session\",\"id\":\"session-{value}\",\"cwd\":\"/tmp/project\",\"timestamp\":\"2026-03-06T10:00:00Z\"}}\n"
                ),
            )
            .expect("write session");
            paths.push(path);
        }
        std::fs::write(
            sessions_dir.join("sessions.json"),
            serde_json::to_vec(&index).expect("json"),
        )
        .expect("write index");

        let display_names = DisplayNameIndexCache::new();
        std::thread::scope(|scope| {
            let display_names = &display_names;
            for chunk in paths.chunks(32) {
                scope.spawn(move || {
                    for path in chunk {
                        let meta = parse_meta(path, display_names, &|| false)
                            .expect("parse session metadata");
                        assert!(meta
                            .title
                            .as_deref()
                            .is_some_and(|title| title.starts_with("name-")));
                    }
                });
            }
        });
        assert_eq!(
            display_names.index_file_reads(),
            1,
            "one scan-scoped cache must not reopen the index for each JSONL"
        );
    }

    #[test]
    fn oversized_display_name_index_is_not_opened_and_rows_still_parse() {
        let temp = tempdir().expect("tempdir");
        let session_path = temp.path().join("session-large.jsonl");
        std::fs::write(
            &session_path,
            concat!(
                "{\"type\":\"session\",\"id\":\"session-large\",\"cwd\":\"/tmp/project\",\"timestamp\":\"2026-03-06T10:00:00Z\"}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"fallback title\"},\"timestamp\":\"2026-03-06T10:01:00Z\"}\n"
            ),
        )
        .expect("write session");
        let index = File::create(temp.path().join("sessions.json")).expect("create index");
        index
            .set_len(DISPLAY_NAME_MAX_FILE_BYTES + 1)
            .expect("extend oversized index");

        let display_names = DisplayNameIndexCache::new();
        let meta = parse_meta(&session_path, &display_names, &|| false).expect("parse row");
        assert_eq!(meta.title.as_deref(), Some("fallback title"));
        assert_eq!(
            display_names.index_file_reads(),
            0,
            "metadata size guard must reject the index before opening it"
        );
    }

    #[test]
    fn display_name_entry_and_directory_caps_degrade_to_bounded_empty_indexes() {
        let temp = tempdir().expect("tempdir");
        let capped_dir = temp.path().join("capped");
        std::fs::create_dir(&capped_dir).expect("create capped dir");
        let mut index = serde_json::Map::new();
        for value in 0..=DISPLAY_NAME_MAX_ENTRIES {
            index.insert(
                format!("agent:{value}"),
                serde_json::json!({
                    "sessionId": format!("session-{value}"),
                    "displayName": format!("name-{value}")
                }),
            );
        }
        let encoded = serde_json::to_vec(&index).expect("encode index");
        assert!(encoded.len() as u64 <= DISPLAY_NAME_MAX_FILE_BYTES);
        std::fs::write(capped_dir.join("sessions.json"), encoded).expect("write capped index");

        let display_names = DisplayNameIndexCache::new();
        let names = display_names
            .get(&capped_dir, &|| false)
            .expect("non-cancelled index load");
        assert!(
            names.is_empty(),
            "an over-entry-cap index must not be partial"
        );

        for value in 0..(DISPLAY_NAME_CACHE_DIRS + 3) {
            let dir = temp.path().join(format!("empty-{value}"));
            std::fs::create_dir(&dir).expect("create empty dir");
            display_names
                .get(&dir, &|| false)
                .expect("cache missing optional index");
        }
        assert_eq!(display_names.cached_dir_count(), DISPLAY_NAME_CACHE_DIRS);
    }

    #[test]
    fn display_name_index_load_can_cancel_and_is_not_cached() {
        let temp = tempdir().expect("tempdir");
        let mut index = serde_json::Map::new();
        for value in 0..300 {
            index.insert(
                format!("agent:{value}"),
                serde_json::json!({
                    "sessionId": format!("session-{value}"),
                    "displayName": format!("name-{value}")
                }),
            );
        }
        std::fs::write(
            temp.path().join("sessions.json"),
            serde_json::to_vec(&index).expect("json"),
        )
        .expect("write index");

        let checks = std::sync::atomic::AtomicUsize::new(0);
        let display_names = DisplayNameIndexCache::new();
        let cancelled = display_names.get(temp.path(), &|| {
            checks.fetch_add(1, std::sync::atomic::Ordering::AcqRel) >= 8
        });
        assert!(cancelled.is_none());
        assert!(checks.load(std::sync::atomic::Ordering::Acquire) < 20);
        assert_eq!(display_names.cached_dir_count(), 0);

        let loaded = display_names
            .get(temp.path(), &|| false)
            .expect("retry after cancellation");
        assert_eq!(
            loaded.get("session-250").map(String::as_str),
            Some("name-250")
        );
        assert_eq!(display_names.index_file_reads(), 2);
    }

    #[test]
    fn parse_session_falls_back_to_dir_basename() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session-def.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"session\",\"id\":\"session-def\",\"cwd\":\"/tmp/my-project\",\"timestamp\":\"2026-03-06T10:00:00Z\"}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"timestamp\":\"2026-03-06T10:01:00Z\"}\n"
            ),
        )
        .expect("write");

        let meta = parse_session(&path, None).unwrap();
        // No user message and no displayName → falls back to dir basename
        assert_eq!(meta.title.as_deref(), Some("my-project"));
    }

    #[test]
    fn parse_session_truncates_long_title() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session-trunc.jsonl");
        let long_msg = "a".repeat(200);
        std::fs::write(
            &path,
            format!(
                "{{\"type\":\"session\",\"id\":\"session-trunc\",\"cwd\":\"/tmp/p\",\"timestamp\":\"2026-03-06T10:00:00Z\"}}\n\
                 {{\"type\":\"message\",\"message\":{{\"role\":\"user\",\"content\":\"{long_msg}\"}},\"timestamp\":\"2026-03-06T10:01:00Z\"}}\n",
            ),
        )
        .expect("write");

        let meta = parse_session(&path, None).unwrap();
        let title = meta.title.unwrap();
        assert!(title.len() <= TITLE_MAX_CHARS + 3); // +3 for "..."
        assert!(title.ends_with("..."));
    }

    #[test]
    fn delete_session_updates_index_and_removes_jsonl() {
        let temp = tempdir().expect("tempdir");
        let sessions_dir = temp.path().join("main").join("sessions");
        std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");

        let session_path = sessions_dir.join("session-123.jsonl");
        std::fs::write(
            &session_path,
            concat!(
                "{\"type\":\"session\",\"id\":\"session-123\",\"cwd\":\"/tmp/project\",\"timestamp\":\"2026-03-06T10:00:00Z\"}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"hello\"},\"timestamp\":\"2026-03-06T10:01:00Z\"}\n"
            ),
        )
        .expect("write session");
        std::fs::write(
            sessions_dir.join("sessions.json"),
            format!(
                r#"{{
                  "agent:main:main": {{
                    "sessionId": "session-123",
                    "sessionFile": "{}"
                  }},
                  "agent:main:other": {{
                    "sessionId": "session-456",
                    "sessionFile": "{}/session-456.jsonl"
                  }}
                }}"#,
                session_path.display(),
                sessions_dir.display()
            ),
        )
        .expect("write index");

        delete_session(temp.path(), &session_path, "session-123").expect("delete session");

        assert!(!session_path.exists());
        let updated: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(sessions_dir.join("sessions.json")).expect("read index"),
        )
        .expect("parse index");
        assert!(updated.get("agent:main:main").is_none());
        assert!(updated.get("agent:main:other").is_some());
    }
}
