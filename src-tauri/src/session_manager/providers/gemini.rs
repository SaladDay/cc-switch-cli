use std::ops::ControlFlow;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::session_manager::cache::{self, FileScanTarget};
use crate::session_manager::scan_cache_store::ScanCacheStore;
use crate::session_manager::{
    SearchSnippet, SessionMessage, SessionMessageBatch, SessionMessageBatchBuilder, SessionMeta,
    SessionSearchHit,
};

use super::utils::{
    build_snippet_cancellable, file_modified_ms, parse_timestamp_to_ms, read_prefix_cancellable,
    read_to_string_cancellable, truncate_summary, MAX_METADATA_FILE_BYTES, MAX_METADATA_LINE_BYTES,
};

const PROVIDER_ID: &str = "gemini";

pub fn scan_sessions() -> Vec<SessionMeta> {
    let gemini_dir = crate::gemini_config::get_gemini_dir();
    let tmp_dir = gemini_dir.join("tmp");
    if !tmp_dir.exists() {
        return Vec::new();
    }

    let mut sessions = Vec::new();

    // Iterate over project directories: tmp/<project_name>/chats/session-*.json
    let project_dirs = match std::fs::read_dir(&tmp_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    for entry in project_dirs.flatten() {
        let chats_dir = entry.path().join("chats");
        if !chats_dir.is_dir() {
            continue;
        }

        let chat_files = match std::fs::read_dir(&chats_dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        let project_root_file = entry.path().join(".project_root");
        let project_dir = read_to_string_cancellable(&project_root_file, &|| false)
            .ok()
            .flatten();

        for file_entry in chat_files.flatten() {
            let path = file_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(meta) = parse_session(&path) {
                sessions.push(SessionMeta {
                    project_dir: project_dir.clone(),
                    ..meta
                });
            }
        }
    }

    sessions
}

/// Cache-aware scan over `tmp/<project>/chats/*.json`.
pub(crate) fn scan_sessions_cached(store: &ScanCacheStore, force: bool) -> Vec<SessionMeta> {
    cache::scan_provider_cached(
        store,
        PROVIDER_ID,
        scan_targets(),
        force,
        parse_meta,
        |_| true,
    )
}

pub(crate) fn scan_sessions_progressive(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(&SessionMeta),
) -> Vec<SessionMeta> {
    let targets = scan_targets();
    match store {
        Some(store) => cache::scan_provider_cached_progressive(
            store,
            PROVIDER_ID,
            targets,
            force,
            parse_meta,
            |_| true,
            on_session,
        ),
        None => cache::scan_provider_uncached_progressive(targets, parse_meta, on_session),
    }
}

pub(crate) fn scan_sessions_progressive_cancellable(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(&SessionMeta),
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionMeta>> {
    let targets = scan_targets_in_cancellable(
        &crate::gemini_config::get_gemini_dir().join("tmp"),
        is_cancelled,
    )?;
    match store {
        Some(store) => cache::scan_provider_cached_progressive_cancellable(
            store,
            PROVIDER_ID,
            targets,
            force,
            parse_meta,
            |_| true,
            on_session,
            is_cancelled,
        ),
        None => cache::scan_provider_uncached_progressive_cancellable(
            targets,
            parse_meta,
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
    let tmp_dir = crate::gemini_config::get_gemini_dir().join("tmp");
    cache::stream_file_provider_cancellable(
        store,
        PROVIDER_ID,
        force,
        |path| parse_meta_cancellable(path, is_cancelled),
        |_| true,
        fingerprint_target,
        move |on_target, cancel| visit_stream_targets(&tmp_dir, on_target, cancel),
        on_session,
        is_cancelled,
    )
}

fn visit_stream_targets(
    tmp_dir: &Path,
    on_target: &mut dyn FnMut(FileScanTarget) -> Result<(), cache::StreamScanStop>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), cache::StreamScanStop> {
    if is_cancelled() {
        return Err(cache::StreamScanStop::Cancelled);
    }
    let projects = match std::fs::read_dir(tmp_dir) {
        Ok(projects) => projects,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            log::warn!(
                "authoritative Gemini project walk failed at {}: {error}",
                tmp_dir.display()
            );
            return Err(cache::StreamScanStop::Incomplete);
        }
    };
    for project in projects {
        if is_cancelled() {
            return Err(cache::StreamScanStop::Cancelled);
        }
        let project = project.map_err(|error| {
            log::warn!(
                "authoritative Gemini project entry failed at {}: {error}",
                tmp_dir.display()
            );
            cache::StreamScanStop::Incomplete
        })?;
        let kind = project.file_type().map_err(|error| {
            log::warn!(
                "authoritative Gemini project type failed at {}: {error}",
                project.path().display()
            );
            cache::StreamScanStop::Incomplete
        })?;
        // Nested directory symlinks are intentionally not followed; the logical
        // tmp root itself may still be a symlink and regular chat-file symlinks
        // are handled by the shared flat walker.
        if !kind.is_dir() {
            continue;
        }
        let project_dir = project.path();
        let project_root = project_dir.join(".project_root");
        let mut decorate = |mut target: FileScanTarget| {
            cache::mix_sibling_into_fingerprint_strict(&mut target, &project_root).map_err(
                |error| {
                    log::warn!(
                        "authoritative Gemini project fingerprint failed at {}: {error}",
                        project_root.display()
                    );
                    cache::StreamScanStop::Incomplete
                },
            )?;
            on_target(target)
        };
        cache::visit_targets_flat_cancellable(
            &project_dir.join("chats"),
            "json",
            &mut decorate,
            is_cancelled,
        )?;
    }
    Ok(())
}

fn scan_targets() -> Vec<FileScanTarget> {
    scan_targets_in(&crate::gemini_config::get_gemini_dir().join("tmp"))
}

/// 收集 `tmp/<project>/chats/*.json` 的直属文件。与 `scan_sessions` 一致，只扫
/// `chats/` 单层目录（用平铺收集器而非递归），嵌套子目录里的文件不纳入——否则
/// 缓存路径会展示旧路径看不到的文件，且其旁路 `.project_root` 定位会取错目录。
fn scan_targets_in(tmp_dir: &Path) -> Vec<FileScanTarget> {
    scan_targets_in_cancellable(tmp_dir, &|| false)
        .expect("non-cancellable target scan cannot stop")
}

fn scan_targets_in_cancellable(
    tmp_dir: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<FileScanTarget>> {
    let mut targets = Vec::new();
    if is_cancelled() {
        return None;
    }
    let project_dirs = match std::fs::read_dir(tmp_dir) {
        Ok(entries) => entries,
        Err(_) => return Some(targets),
    };
    for entry in project_dirs.flatten() {
        if is_cancelled() {
            return None;
        }
        let chats_dir = entry.path().join("chats");
        if !chats_dir.is_dir() {
            continue;
        }
        let mut project_targets = Vec::new();
        if !cache::collect_targets_flat_cancellable(
            &chats_dir,
            "json",
            &mut project_targets,
            is_cancelled,
        ) {
            return None;
        }
        // project_dir 派生自旁路的 .project_root：它变化时缓存也必须失效
        let project_root = entry.path().join(".project_root");
        for target in &mut project_targets {
            if is_cancelled() {
                return None;
            }
            cache::mix_sibling_into_fingerprint(target, &project_root);
        }
        targets.extend(project_targets);
    }
    Some(targets)
}

/// Parse one session file, injecting `project_dir` from the sibling
/// `<project>/.project_root` exactly like `scan_sessions` does. The cached
/// `SessionMeta` bakes this in, so unchanged files keep the resolved directory.
fn parse_meta(path: &Path) -> Option<SessionMeta> {
    let mut meta = parse_session(path)?;
    // path: tmp/<project>/chats/<session>.json → .project_root at tmp/<project>/
    meta.project_dir = path
        .parent()
        .and_then(Path::parent)
        .map(|project| project.join(".project_root"))
        .and_then(|file| read_to_string_cancellable(&file, &|| false).ok().flatten());
    Some(meta)
}

fn fingerprint_target(path: &Path) -> Option<FileScanTarget> {
    let mut target = cache::stat_target_strict(path).ok().flatten()?;
    let project_root = path
        .parent()
        .and_then(Path::parent)
        .map(|project| project.join(".project_root"))?;
    cache::mix_sibling_into_fingerprint_strict(&mut target, &project_root).ok()?;
    Some(target)
}

fn parse_meta_cancellable(
    path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<SessionMeta>, cache::StreamScanStop> {
    let parsed = match read_to_string_cancellable(path, is_cancelled) {
        Ok(Some(data)) => parse_session_data(path, &data),
        Ok(None) => return Err(cache::StreamScanStop::Cancelled),
        Err(error) if error.kind() == std::io::ErrorKind::FileTooLarge => {
            let prefix = read_prefix_cancellable(path, MAX_METADATA_LINE_BYTES, is_cancelled)
                .map_err(|error| {
                    log::warn!(
                        "authoritative oversized Gemini metadata fallback failed at {}: {error}",
                        path.display()
                    );
                    cache::StreamScanStop::Incomplete
                })?
                .ok_or(cache::StreamScanStop::Cancelled)?;
            Some(parse_session_lightweight(path, &prefix))
        }
        Err(error) => {
            log::warn!(
                "authoritative Gemini metadata read failed at {}: {error}",
                path.display()
            );
            return Err(cache::StreamScanStop::Incomplete);
        }
    };
    if is_cancelled() {
        return Err(cache::StreamScanStop::Cancelled);
    }
    let mut meta = parsed.unwrap_or_else(|| parse_session_lightweight(path, ""));
    let project_root = path
        .parent()
        .and_then(Path::parent)
        .map(|project| project.join(".project_root"));
    meta.project_dir = match project_root {
        Some(file) => match read_to_string_cancellable(&file, is_cancelled) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) if error.kind() == std::io::ErrorKind::FileTooLarge => {
                read_prefix_cancellable(&file, MAX_METADATA_LINE_BYTES, is_cancelled).map_err(
                    |error| {
                        log::warn!(
                            "authoritative Gemini project fallback failed at {}: {error}",
                            file.display()
                        );
                        cache::StreamScanStop::Incomplete
                    },
                )?
            }
            Err(error) => {
                log::warn!(
                    "authoritative Gemini project read failed at {}: {error}",
                    file.display()
                );
                return Err(cache::StreamScanStop::Incomplete);
            }
        },
        None => None,
    };
    if is_cancelled() {
        Err(cache::StreamScanStop::Cancelled)
    } else {
        Ok(Some(meta))
    }
}

fn parse_session_lightweight(path: &Path, prefix: &str) -> SessionMeta {
    let session_id = extract_json_value(prefix, "sessionId")
        .and_then(|value| value.as_str().map(str::to_owned))
        .or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let created_at =
        extract_json_value(prefix, "startTime").and_then(|value| parse_timestamp_to_ms(&value));
    let last_active_at = extract_json_value(prefix, "lastUpdated")
        .and_then(|value| parse_timestamp_to_ms(&value))
        .or_else(|| file_modified_ms(path));
    let title = extract_json_value(prefix, "content")
        .and_then(|value| value.as_str().map(|text| truncate_summary(text, 160)))
        .or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .map(|value| truncate_summary(value, 160))
        });
    SessionMeta {
        provider_id: PROVIDER_ID.to_string(),
        session_id: session_id.clone(),
        title: title.clone(),
        summary: title,
        project_dir: None,
        created_at: created_at.or(last_active_at),
        last_active_at,
        source_path: Some(path.to_string_lossy().into_owned()),
        resume_command: Some(format!("gemini --resume {session_id}")),
    }
}

fn extract_json_value(input: &str, key: &str) -> Option<Value> {
    let needle = format!("\"{key}\"");
    for (offset, _) in input.match_indices(&needle) {
        let tail = input.get(offset + needle.len()..)?;
        let value_start = tail.find(':')? + 1;
        let mut deserializer = serde_json::Deserializer::from_str(tail[value_start..].trim_start());
        if let Ok(value) = Value::deserialize(&mut deserializer) {
            return Some(value);
        }
    }
    None
}

pub fn load_messages(path: &Path) -> Result<SessionMessageBatch, String> {
    load_messages_cancellable(path, &|| false)
}

pub(crate) fn load_messages_cancellable(
    path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<SessionMessageBatch, String> {
    let data = match read_to_string_cancellable(path, is_cancelled) {
        Ok(Some(data)) => data,
        Ok(None) => return Err("Session message preview was cancelled".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::FileTooLarge => {
            return Err(format!(
                "Gemini session exceeds the {} MiB bounded preview limit",
                MAX_METADATA_FILE_BYTES / (1024 * 1024)
            ));
        }
        Err(error) => return Err(format!("Failed to read bounded session payload: {error}")),
    };
    let value: Value =
        serde_json::from_str(&data).map_err(|e| format!("Failed to parse session JSON: {e}"))?;

    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "No messages array found".to_string())?;

    let mut batch = SessionMessageBatchBuilder::new();
    for msg in messages {
        if is_cancelled() {
            return Err("Session message preview was cancelled".to_string());
        }
        let role = match msg.get("type").and_then(Value::as_str) {
            Some("gemini") => "assistant",
            Some("user") => "user",
            Some("info") | Some("error") => continue,
            Some(_) | None => continue,
        };

        // Gemini content may be a plain string or an array of {text: ...} objects
        let mut content = match msg.get("content") {
            Some(Value::String(s)) => s.to_string(),
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };

        // Append tool call names from the optional toolCalls array
        if let Some(Value::Array(calls)) = msg.get("toolCalls") {
            for call in calls {
                if let Some(name) = call.get("name").and_then(Value::as_str) {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str(&format!("[Tool: {name}]"));
                }
            }
        }

        if content.trim().is_empty() {
            continue;
        }

        let ts = msg.get("timestamp").and_then(parse_timestamp_to_ms);

        if batch
            .push(SessionMessage {
                role: role.to_string(),
                content,
                ts,
            })
            .is_break()
        {
            break;
        }
    }

    Ok(batch.finish())
}

/// Search a single Gemini session JSON file for `needle` (case-insensitive).
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
    let data = read_to_string_cancellable(path, is_cancelled).ok()??;
    if is_cancelled() {
        return None;
    }
    let value: Value = serde_json::from_str(&data).ok()?;
    if is_cancelled() {
        return None;
    }
    let messages = value.get("messages").and_then(Value::as_array)?;
    let mut snippets: Vec<SearchSnippet> = Vec::new();
    const MAX_SNIPPETS: usize = 5;

    for msg in messages {
        if is_cancelled() {
            return None;
        }
        let role = match msg.get("type").and_then(Value::as_str) {
            Some("gemini") => "assistant",
            Some("user") => "user",
            _ => continue,
        };
        let mut content = match msg.get("content") {
            Some(Value::String(s)) => s.to_string(),
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        if let Some(Value::Array(calls)) = msg.get("toolCalls") {
            for call in calls {
                if let Some(name) = call.get("name").and_then(Value::as_str) {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str(&format!("[Tool: {name}]"));
                }
            }
        }
        if content.trim().is_empty() {
            continue;
        }
        if let Some(snippet) = build_snippet_cancellable(&content, needle, is_cancelled).ok()? {
            snippets.push(SearchSnippet {
                role: role.to_string(),
                snippet,
            });
            if snippets.len() >= MAX_SNIPPETS {
                break;
            }
        }
    }
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
    let meta = parse_session(path).ok_or_else(|| {
        format!(
            "Failed to parse Gemini session metadata: {}",
            path.display()
        )
    })?;

    if meta.session_id != session_id {
        return Err(format!(
            "Gemini session ID mismatch: expected {session_id}, found {}",
            meta.session_id
        ));
    }

    std::fs::remove_file(path).map_err(|e| {
        format!(
            "Failed to delete Gemini session file {}: {e}",
            path.display()
        )
    })?;

    Ok(true)
}

fn parse_session(path: &Path) -> Option<SessionMeta> {
    match read_to_string_cancellable(path, &|| false) {
        Ok(Some(data)) => parse_session_data(path, &data),
        Err(error) if error.kind() == std::io::ErrorKind::FileTooLarge => {
            let prefix = read_prefix_cancellable(path, MAX_METADATA_LINE_BYTES, &|| false)
                .ok()
                .flatten()?;
            Some(parse_session_lightweight(path, &prefix))
        }
        Ok(None) | Err(_) => None,
    }
}

fn parse_session_data(path: &Path, data: &str) -> Option<SessionMeta> {
    let value: Value = serde_json::from_str(&data).ok()?;

    let session_id = value.get("sessionId").and_then(Value::as_str)?.to_string();

    let created_at = value.get("startTime").and_then(parse_timestamp_to_ms);
    let last_active_at = value.get("lastUpdated").and_then(parse_timestamp_to_ms);

    // Derive title from first user message
    let title = value
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|msgs| {
            msgs.iter()
                .find(|m| m.get("type").and_then(Value::as_str) == Some("user"))
                .and_then(|m| m.get("content").and_then(Value::as_str))
                .filter(|s| !s.trim().is_empty())
                .map(|s| truncate_summary(s, 160))
        });

    let source_path = path.to_string_lossy().to_string();

    Some(SessionMeta {
        provider_id: PROVIDER_ID.to_string(),
        session_id: session_id.clone(),
        title: title.clone(),
        summary: title,
        project_dir: None, // (optionally) populated later
        created_at,
        last_active_at: last_active_at.or(created_at),
        source_path: Some(source_path),
        resume_command: Some(format!("gemini --resume {session_id}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn delete_session_removes_json_file() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session-2026-03-06T10-17-test.json");
        std::fs::write(
            &path,
            r#"{
              "sessionId": "gemini-session-123",
              "startTime": "2026-03-06T10:17:58.000Z",
              "lastUpdated": "2026-03-06T10:20:00.000Z",
              "messages": [
                {
                  "id": "msg-1",
                  "timestamp": "2026-03-06T10:17:58.000Z",
                  "type": "user",
                  "content": "hello"
                }
              ]
            }"#,
        )
        .expect("write session");

        delete_session(temp.path(), &path, "gemini-session-123").expect("delete session");

        assert!(!path.exists());
    }

    #[test]
    fn scan_targets_ignores_nested_chat_subdirectories() {
        let temp = tempdir().expect("tempdir");
        let tmp_dir = temp.path();
        let chats = tmp_dir.join("proj").join("chats");
        std::fs::create_dir_all(chats.join("archive")).expect("mkdir");
        // chats/ 直属会话文件应被收集
        std::fs::write(chats.join("session-1.json"), "{}").expect("write");
        // 嵌套子目录里的会话文件不应被缓存扫描收集
        std::fs::write(chats.join("archive").join("session-2.json"), "{}").expect("write");

        let targets = scan_targets_in(tmp_dir);
        assert_eq!(targets.len(), 1, "只收集 chats/ 直属文件");
        assert!(targets
            .iter()
            .any(|t| t.path.file_name().and_then(|n| n.to_str()) == Some("session-1.json")));
        assert!(!targets
            .iter()
            .any(|t| t.path.file_name().and_then(|n| n.to_str()) == Some("session-2.json")));
    }

    #[test]
    fn oversized_session_uses_bounded_metadata_fallback() {
        let temp = tempdir().expect("tempdir");
        let chats = temp.path().join("project").join("chats");
        std::fs::create_dir_all(&chats).expect("mkdir");
        let path = chats.join("session.json");
        std::fs::write(
            &path,
            br#"{"sessionId":"bounded-id","startTime":1700000000,"messages":[]}"#,
        )
        .expect("write prefix");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open")
            .set_len((MAX_METADATA_FILE_BYTES + 1) as u64)
            .expect("extend sparse file");

        let meta = parse_meta_cancellable(&path, &|| false)
            .expect("authoritative parse")
            .expect("fallback metadata");
        let expected_source = path.to_string_lossy().into_owned();
        assert_eq!(meta.session_id, "bounded-id");
        assert_eq!(meta.source_path.as_deref(), Some(expected_source.as_str()));

        let error = load_messages(&path).expect_err("oversized detail must stay bounded");
        assert!(error.contains("4 MiB bounded preview limit"));
    }

    #[test]
    fn load_messages_handles_array_content() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session.json");
        std::fs::write(
            &path,
            r#"{
              "sessionId": "test",
              "messages": [
                {"id":"1","timestamp":"2026-03-06T10:00:00Z","type":"user","content":[{"text":"hello"}]},
                {"id":"2","timestamp":"2026-03-06T10:00:01Z","type":"gemini","content":"world"},
                {"id":"3","timestamp":"2026-03-06T10:00:02Z","type":"info","content":"system info"},
                {"id":"4","timestamp":"2026-03-06T10:00:03Z","type":"error","content":"MCP ERROR"}
              ]
            }"#,
        )
        .expect("write");

        let msgs = load_messages(&path).expect("load");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "hello");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].content, "world");
    }

    #[test]
    fn load_messages_includes_tool_calls() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session.json");
        std::fs::write(
            &path,
            r#"{
              "sessionId": "test",
              "messages": [
                {"id":"1","timestamp":"2026-03-10T08:24:50Z","type":"gemini","content":"","toolCalls":[{"id":"call_1","name":"web_search","args":{"query":"test"}}]},
                {"id":"2","timestamp":"2026-03-10T08:25:00Z","type":"gemini","content":"Here are the results.","toolCalls":[{"id":"call_2","name":"web_fetch","args":{"url":"http://example.com"}}]}
              ]
            }"#,
        )
        .expect("write");

        let msgs = load_messages(&path).expect("load");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "assistant");
        assert!(msgs[0].content.contains("[Tool: web_search]"));
        assert_eq!(msgs[1].role, "assistant");
        assert!(msgs[1].content.contains("Here are the results."));
        assert!(msgs[1].content.contains("[Tool: web_fetch]"));
    }
}
