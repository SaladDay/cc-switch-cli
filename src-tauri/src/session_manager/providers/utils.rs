use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, FixedOffset};
use serde_json::Value;

use crate::session_manager::SessionMeta;

/// Parse many session files in parallel, preserving input order. Each file is
/// read independently, so this is a pure fan-out over `parse`. Falls back to a
/// serial pass for small inputs where thread setup would not pay off. This keeps
/// the first scan of a provider with tens of thousands of files (e.g. Claude
/// under `~/.claude/projects`) from blocking for tens of seconds.
///
/// Parallelism is deliberately capped at roughly half the cores: this scan runs
/// on a background worker while a single-threaded ratatui UI loop needs CPU to
/// stay responsive. Using every core (and the allocator churn of building tens
/// of thousands of `SessionMeta`) starves the UI thread and makes key input
/// feel frozen, so we leave clear headroom and accept a slightly slower scan.
pub fn parse_sessions_parallel<F>(files: Vec<PathBuf>, parse: F) -> Vec<SessionMeta>
where
    F: Fn(&Path) -> Option<SessionMeta> + Sync,
{
    let workers = std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(2)
        .min(4);
    if workers <= 1 || files.len() < 64 {
        return files.iter().filter_map(|path| parse(path)).collect();
    }
    let chunk_size = files.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(|| {
                    chunk
                        .iter()
                        .filter_map(|path| parse(path))
                        .collect::<Vec<SessionMeta>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .flatten()
            .collect()
    })
}

/// Maximum number of characters for session titles (shared across providers).
pub const TITLE_MAX_CHARS: usize = 80;

/// Hard byte ceilings for list/search metadata reads. Four parser workers may
/// run concurrently, so these limits also bound aggregate transient memory.
pub(crate) const MAX_METADATA_FILE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_METADATA_LINE_BYTES: usize = 256 * 1024;

pub(crate) fn file_modified_ms(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

/// Bounded head/tail reader for authoritative metadata scans. It never retains
/// more than two fixed windows even if a JSONL record itself is enormous.
pub(crate) fn read_head_tail_lines_bounded(
    path: &Path,
    head_n: usize,
    tail_n: usize,
) -> io::Result<(Vec<String>, Vec<String>)> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let window = MAX_METADATA_LINE_BYTES as u64;

    let mut head_bytes = Vec::with_capacity(MAX_METADATA_LINE_BYTES);
    (&mut file).take(window).read_to_end(&mut head_bytes)?;
    if file_len > window && head_bytes.last().copied() != Some(b'\n') {
        if let Some(last_newline) = head_bytes.iter().rposition(|byte| *byte == b'\n') {
            head_bytes.truncate(last_newline + 1);
        } else {
            head_bytes.clear();
        }
    }
    let head = String::from_utf8_lossy(&head_bytes)
        .lines()
        .take(head_n)
        .map(str::to_owned)
        .collect();

    let seek_pos = file_len.saturating_sub(window);
    file.seek(SeekFrom::Start(seek_pos))?;
    let mut tail_bytes = Vec::with_capacity(MAX_METADATA_LINE_BYTES);
    file.take(window).read_to_end(&mut tail_bytes)?;
    let mut tail_lines = String::from_utf8_lossy(&tail_bytes)
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if seek_pos > 0 && !tail_bytes.starts_with(b"\n") && !tail_lines.is_empty() {
        tail_lines.remove(0);
    }
    let tail_from = tail_lines.len().saturating_sub(tail_n);
    let tail = tail_lines.into_iter().skip(tail_from).collect();
    Ok((head, tail))
}

/// Read the first `head_n` lines and last `tail_n` lines from a file.
/// For small files (< 16 KB), reads all lines once to avoid unnecessary seeking.
#[cfg(test)]
pub fn read_head_tail_lines(
    path: &Path,
    head_n: usize,
    tail_n: usize,
) -> io::Result<(Vec<String>, Vec<String>)> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();

    // For small files, read all lines once and split
    if file_len < 16_384 {
        let reader = BufReader::new(&file);
        let all: Vec<String> = reader.lines().map_while(Result::ok).collect();
        let head = all.iter().take(head_n).cloned().collect();
        let skip = all.len().saturating_sub(tail_n);
        let tail = all.into_iter().skip(skip).collect();
        return Ok((head, tail));
    }

    // Read head lines from the beginning. Borrow the handle so the same open file
    // can be seeked for the tail below instead of reopening it a second time.
    let head: Vec<String> = {
        let reader = BufReader::new(&file);
        reader.lines().take(head_n).map_while(Result::ok).collect()
    };

    // Seek to last ~16 KB for tail lines, reusing the same file handle.
    let seek_pos = file_len.saturating_sub(16_384);
    file.seek(SeekFrom::Start(seek_pos))?;
    let all_tail: Vec<String> = BufReader::new(&file)
        .lines()
        .map_while(Result::ok)
        .collect();

    // Skip first partial line if we seeked into the middle of a line
    let skip_first = if seek_pos > 0 { 1 } else { 0 };
    let usable: Vec<String> = all_tail.into_iter().skip(skip_first).collect();
    let skip = usable.len().saturating_sub(tail_n);
    let tail = usable.into_iter().skip(skip).collect();

    Ok((head, tail))
}

/// Read a file in bounded chunks so a superseded deep search can stop during
/// large JSON reads instead of waiting for `read_to_string` to reach EOF.
pub(crate) fn read_to_string_cancellable(
    path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> io::Result<Option<String>> {
    let mut file = File::open(path)?;
    let declared_len = file
        .metadata()
        .ok()
        .and_then(|meta| usize::try_from(meta.len()).ok())
        .unwrap_or(0);
    if declared_len > MAX_METADATA_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!(
                "metadata file exceeds {} byte limit",
                MAX_METADATA_FILE_BYTES
            ),
        ));
    }
    let initial_capacity = declared_len.min(MAX_METADATA_FILE_BYTES);
    let mut bytes = Vec::with_capacity(initial_capacity);
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        if is_cancelled() {
            return Ok(None);
        }
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) > MAX_METADATA_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                format!(
                    "metadata file grew beyond {} byte limit",
                    MAX_METADATA_FILE_BYTES
                ),
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    if is_cancelled() {
        return Ok(None);
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

pub(crate) fn read_prefix_cancellable(
    path: &Path,
    max_bytes: usize,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> io::Result<Option<String>> {
    let mut file = File::open(path)?;
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    let mut chunk = [0_u8; 64 * 1024];
    while bytes.len() < max_bytes {
        if is_cancelled() {
            return Ok(None);
        }
        let wanted = chunk.len().min(max_bytes - bytes.len());
        let read = file.read(&mut chunk[..wanted])?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    if is_cancelled() {
        Ok(None)
    } else {
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }
}

/// Visit UTF-8 lines without ever allocating more than one bounded record.
/// Oversized records are drained and skipped; this keeps deep search safe from
/// a single giant JSONL/tool-output line while preserving later records.
pub(crate) fn visit_bounded_lines_cancellable(
    path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    on_line: &mut dyn FnMut(&str) -> ControlFlow<()>,
) -> io::Result<Option<()>> {
    visit_bounded_lines_cancellable_with_status(path, is_cancelled, on_line)
        .map(|outcome| outcome.map(|_| ()))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BoundedLineVisit {
    pub oversized_record_skipped: bool,
}

pub(crate) fn visit_bounded_lines_cancellable_with_status(
    path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    on_line: &mut dyn FnMut(&str) -> ControlFlow<()>,
) -> io::Result<Option<BoundedLineVisit>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::with_capacity(8 * 1024);
    let mut oversized = false;
    let mut status = BoundedLineVisit::default();
    loop {
        if is_cancelled() {
            return Ok(None);
        }
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            if !line.is_empty() && !oversized {
                let text = String::from_utf8_lossy(&line);
                let _ = on_line(text.trim_end_matches('\r'));
            }
            return Ok(Some(status));
        }
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(buffer.len(), |index| index + 1);
        if !oversized {
            let content_len = take.saturating_sub(usize::from(newline.is_some()));
            if line.len().saturating_add(content_len) <= MAX_METADATA_LINE_BYTES {
                line.extend_from_slice(&buffer[..content_len]);
            } else {
                line.clear();
                oversized = true;
                status.oversized_record_skipped = true;
            }
        }
        reader.consume(take);
        if newline.is_some() {
            if !oversized {
                let text = String::from_utf8_lossy(&line);
                if on_line(text.trim_end_matches('\r')).is_break() {
                    return Ok(Some(status));
                }
            }
            line.clear();
            oversized = false;
        }
    }
}

/// Run SQLite work while a scoped monitor turns cooperative cancellation into
/// `sqlite3_interrupt()`. This covers a single expensive `sqlite3_step` (for
/// example an unindexed ORDER BY) where row-boundary checks cannot run yet.
pub(crate) fn with_sqlite_cancellation<T>(
    conn: &rusqlite::Connection,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    query: impl FnOnce() -> T,
) -> T {
    struct QueryDone<'a>(&'a AtomicBool);

    impl Drop for QueryDone<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    let done = AtomicBool::new(false);
    let interrupt = conn.get_interrupt_handle();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Acquire) {
                if is_cancelled() {
                    interrupt.interrupt();
                    break;
                }
                std::thread::park_timeout(std::time::Duration::from_millis(1));
            }
        });
        let _done = QueryDone(&done);
        query()
    })
}

pub fn parse_timestamp_to_ms(value: &Value) -> Option<i64> {
    // Integer: milliseconds (>1e12) or seconds
    if let Some(n) = value.as_i64() {
        return Some(if n > 1_000_000_000_000 { n } else { n * 1000 });
    }
    if let Some(n) = value.as_f64() {
        let n = n as i64;
        return Some(if n > 1_000_000_000_000 { n } else { n * 1000 });
    }
    // RFC3339 string
    let raw = value.as_str()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt: DateTime<FixedOffset>| dt.timestamp_millis())
}

pub fn extract_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.to_string(),
        Value::Array(items) => items
            .iter()
            .filter_map(extract_text_from_item)
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => map
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

fn extract_text_from_item(item: &Value) -> Option<String> {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");

    // tool_use: show tool name
    if item_type == "tool_use" {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Some(format!("[Tool: {name}]"));
    }

    // tool_result: extract nested content
    if item_type == "tool_result" {
        if let Some(content) = item.get("content") {
            let text = extract_text(content);
            if !text.is_empty() {
                return Some(text);
            }
        }
        return None;
    }

    if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }

    if let Some(text) = item.get("input_text").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }

    if let Some(text) = item.get("output_text").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }

    if let Some(content) = item.get("content") {
        let text = extract_text(content);
        if !text.is_empty() {
            return Some(text);
        }
    }

    None
}

pub fn truncate_summary(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }

    let mut result = trimmed.chars().take(max_chars).collect::<String>();
    result.push_str("...");
    result
}

pub fn path_basename(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = trimmed.trim_end_matches(['/', '\\']);
    let last = normalized
        .split(['/', '\\'])
        .next_back()
        .filter(|segment| !segment.is_empty())?;
    Some(last.to_string())
}

/// Maximum number of characters in a search snippet (context around a match).
pub const SNIPPET_MAX_CHARS: usize = 160;

/// Maximum amount of transcript text inspected between cooperative
/// cancellation checks while looking for a deep-search match.
const SNIPPET_CANCEL_CHECK_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SnippetSearchCancelled;

/// Build a search snippet around the first occurrence of `needle` in `haystack`.
/// Returns up to `SNIPPET_MAX_CHARS` chars, centered on the match when possible.
/// Matching uses Unicode lowercase scalars and preserves the original text.
#[allow(dead_code)]
pub fn build_snippet(haystack: &str, needle: &str) -> Option<String> {
    build_snippet_cancellable(haystack, needle, &|| false)
        .ok()
        .flatten()
}

/// Cancellation-aware, linear-time snippet search.
///
/// The previous implementation rebuilt and lowercased one `needle`-sized
/// `String` at every character offset, making a single long message
/// `O(haystack * needle)` and temporarily allocation-heavy. This version uses
/// a KMP prefix table over Unicode lowercase scalars. It visits the haystack
/// once, retains only the query-sized matcher state plus the bounded context
/// window, and checks cancellation at least every 4 KiB. Byte boundaries saved
/// from `char_indices` keep the returned slice valid UTF-8 even when lowercase
/// expansion maps one source character to multiple scalars.
pub(crate) fn build_snippet_cancellable(
    haystack: &str,
    needle: &str,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<String>, SnippetSearchCancelled> {
    if is_cancelled() {
        return Err(SnippetSearchCancelled);
    }
    if needle.is_empty() {
        return Ok(None);
    }

    let mut folded_needle = Vec::with_capacity(needle.len().min(SNIPPET_MAX_CHARS));
    let mut needle_chars = 0usize;
    let mut next_cancel_at = SNIPPET_CANCEL_CHECK_BYTES;
    for (byte_index, ch) in needle.char_indices() {
        if byte_index >= next_cancel_at {
            if is_cancelled() {
                return Err(SnippetSearchCancelled);
            }
            next_cancel_at = byte_index.saturating_add(SNIPPET_CANCEL_CHECK_BYTES);
        }
        needle_chars = needle_chars.saturating_add(1);
        folded_needle.extend(ch.to_lowercase());
    }
    if folded_needle.is_empty() {
        return Ok(None);
    }

    // KMP prefix construction is itself cancellable for a deliberately huge
    // query. Its memory is proportional only to the query, never the corpus.
    let mut prefix = vec![0usize; folded_needle.len()];
    for index in 1..folded_needle.len() {
        if index % SNIPPET_CANCEL_CHECK_BYTES == 0 && is_cancelled() {
            return Err(SnippetSearchCancelled);
        }
        let mut matched = prefix[index - 1];
        while matched > 0 && folded_needle[index] != folded_needle[matched] {
            matched = prefix[matched - 1];
        }
        if folded_needle[index] == folded_needle[matched] {
            matched += 1;
        }
        prefix[index] = matched;
    }

    // The token deque identifies the source character where a normalized KMP
    // match began. The character deque supplies at most half a snippet of
    // leading context without rescanning the haystack.
    let token_capacity = folded_needle.len();
    let context_capacity = token_capacity
        .saturating_add(SNIPPET_MAX_CHARS)
        .saturating_add(1);
    if is_cancelled() {
        return Err(SnippetSearchCancelled);
    }
    let mut recent_tokens: VecDeque<(usize, usize)> = VecDeque::with_capacity(token_capacity);
    let mut recent_chars: VecDeque<(usize, usize)> = VecDeque::with_capacity(context_capacity);
    let mut matched = 0usize;
    let mut found: Option<(usize, usize, usize)> = None;
    let mut end_byte = haystack.len();
    next_cancel_at = 0;

    'haystack: for (ordinal, (byte_index, ch)) in haystack.char_indices().enumerate() {
        if byte_index >= next_cancel_at {
            if is_cancelled() {
                return Err(SnippetSearchCancelled);
            }
            next_cancel_at = byte_index.saturating_add(SNIPPET_CANCEL_CHECK_BYTES);
        }

        if let Some((_, _, target_end_ordinal)) = found {
            if ordinal >= target_end_ordinal {
                end_byte = byte_index;
                break;
            }
            continue;
        }

        recent_chars.push_back((ordinal, byte_index));
        if recent_chars.len() > context_capacity {
            recent_chars.pop_front();
        }

        for folded in ch.to_lowercase() {
            recent_tokens.push_back((ordinal, byte_index));
            if recent_tokens.len() > token_capacity {
                recent_tokens.pop_front();
            }

            while matched > 0 && folded != folded_needle[matched] {
                matched = prefix[matched - 1];
            }
            if folded == folded_needle[matched] {
                matched += 1;
            }
            if matched != folded_needle.len() {
                continue;
            }

            let (match_start_ordinal, _) = recent_tokens
                .front()
                .copied()
                .expect("a complete KMP match retains its first token");
            let match_end_ordinal = ordinal.saturating_add(1);
            let match_chars = match_end_ordinal.saturating_sub(match_start_ordinal);
            let window_chars = SNIPPET_MAX_CHARS.max(needle_chars).max(match_chars);
            let leading_chars = window_chars.saturating_sub(match_chars) / 2;
            let context_start_ordinal = match_start_ordinal.saturating_sub(leading_chars);
            let context_start_byte = recent_chars
                .iter()
                .find_map(|(seen_ordinal, seen_byte)| {
                    (*seen_ordinal == context_start_ordinal).then_some(*seen_byte)
                })
                .unwrap_or(0);
            let target_end_ordinal = context_start_ordinal.saturating_add(window_chars);
            found = Some((
                context_start_byte,
                context_start_ordinal,
                target_end_ordinal,
            ));

            if match_end_ordinal >= target_end_ordinal {
                end_byte = byte_index.saturating_add(ch.len_utf8());
                break 'haystack;
            }
            break;
        }
    }

    if is_cancelled() {
        return Err(SnippetSearchCancelled);
    }
    let Some((start_byte, start_ordinal, _)) = found else {
        return Ok(None);
    };
    let body = haystack[start_byte..end_byte].trim();
    let mut snippet = String::with_capacity(
        body.len()
            .saturating_add(usize::from(start_ordinal > 0))
            .saturating_add(usize::from(end_byte < haystack.len())),
    );
    if start_ordinal > 0 {
        snippet.push('…');
    }
    snippet.push_str(body);
    if end_byte < haystack.len() {
        snippet.push('…');
    }
    Ok(Some(snippet))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn read_head_tail_small_file_reads_all_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("small.jsonl");
        let mut f = File::create(&path).expect("create");
        for i in 0..8 {
            writeln!(f, "line-{i}").expect("write");
        }
        drop(f);

        let (head, tail) = read_head_tail_lines(&path, 3, 2).expect("read");
        assert_eq!(head, vec!["line-0", "line-1", "line-2"]);
        assert_eq!(tail, vec!["line-6", "line-7"]);
    }

    #[test]
    fn read_head_tail_large_file_seeks_for_tail() {
        // Build a file well over the 16 KB threshold so the seek path runs.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("large.jsonl");
        let mut f = File::create(&path).expect("create");
        let total = 4_000; // each line ~12 bytes → ~48 KB
        for i in 0..total {
            writeln!(f, "line-{i:07}").expect("write");
        }
        drop(f);

        let (head, tail) = read_head_tail_lines(&path, 4, 3).expect("read");
        assert_eq!(
            head,
            vec![
                "line-0000000",
                "line-0000001",
                "line-0000002",
                "line-0000003"
            ]
        );
        // The tail must be the genuine last lines, and the seek-into-a-partial-line
        // handling must never leak a truncated fragment.
        assert_eq!(tail, vec!["line-0003997", "line-0003998", "line-0003999"]);
        for line in head.iter().chain(tail.iter()) {
            assert!(line.starts_with("line-") && line.len() == "line-0000000".len());
        }
    }

    #[test]
    fn read_head_tail_large_file_matches_full_read() {
        // Cross-check the seek path against a naive full read of the same file.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cmp.jsonl");
        let mut f = File::create(&path).expect("create");
        for i in 0..5_000 {
            // Vary line width so a fixed 16 KB seek lands mid-line.
            writeln!(f, "row{i}-{}", "x".repeat(i % 7)).expect("write");
        }
        drop(f);

        let all: Vec<String> = std::io::BufReader::new(File::open(&path).expect("open"))
            .lines()
            .map_while(Result::ok)
            .collect();
        let expected_head: Vec<String> = all.iter().take(10).cloned().collect();
        let expected_tail: Vec<String> = all
            .iter()
            .skip(all.len().saturating_sub(20))
            .cloned()
            .collect();

        let (head, tail) = read_head_tail_lines(&path, 10, 20).expect("read");
        assert_eq!(head, expected_head);
        assert_eq!(tail, expected_tail);
    }

    #[test]
    fn bounded_line_visitor_skips_one_oversized_record_and_keeps_following_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("oversized.jsonl");
        let mut file = File::create(&path).expect("create");
        file.write_all(&vec![b'x'; MAX_METADATA_LINE_BYTES + 1])
            .expect("large line");
        file.write_all(b"\nsmall-match\n").expect("tail");
        drop(file);

        let mut lines = Vec::new();
        visit_bounded_lines_cancellable(&path, &|| false, &mut |line| {
            lines.push(line.to_string());
            ControlFlow::Continue(())
        })
        .expect("visit")
        .expect("not cancelled");
        assert_eq!(lines, vec!["small-match"]);
    }

    #[test]
    fn cancellable_string_reader_rejects_sparse_oversized_file_before_allocation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("huge.json");
        let file = File::create(&path).expect("create");
        file.set_len((MAX_METADATA_FILE_BYTES + 1) as u64)
            .expect("set len");
        let error = read_to_string_cancellable(&path, &|| false).expect_err("must reject");
        assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
    }

    #[test]
    fn parse_timestamp_to_ms_supports_integers_and_rfc3339() {
        assert_eq!(
            parse_timestamp_to_ms(&json!(1_771_061_953_033_i64)),
            Some(1_771_061_953_033)
        );
        assert_eq!(
            parse_timestamp_to_ms(&json!(1_771_061_953_i64)),
            Some(1_771_061_953_000)
        );
        assert_eq!(
            parse_timestamp_to_ms(&json!("1970-01-01T00:00:01Z")),
            Some(1_000)
        );
    }

    #[test]
    fn build_snippet_finds_ascii_substring_case_insensitively() {
        let haystack = "The quick BROWN fox jumps over the lazy dog";
        let s = build_snippet(haystack, "brown").expect("should find 'brown'");
        assert!(s.to_lowercase().contains("brown"));
    }

    #[test]
    fn build_snippet_finds_cjk_substring() {
        let haystack =
            "这是一段关于浙江移动的对话内容，后面还有很多其他文字用于测试截断逻辑是否正确工作。";
        let s = build_snippet(haystack, "浙江移动").expect("should find CJK");
        assert!(s.contains("浙江移动"));
    }

    #[test]
    fn build_snippet_preserves_utf8_boundaries_with_unicode_case_matching() {
        let haystack = "🙂前缀 ÉCLAIR 浙江移动 后缀🙂";
        let snippet = build_snippet(haystack, "éclair").expect("Unicode case match");

        assert!(snippet.contains("ÉCLAIR"));
        assert!(std::str::from_utf8(snippet.as_bytes()).is_ok());
    }

    #[test]
    fn build_snippet_finds_needle_longer_than_max_chars() {
        // A query longer than SNIPPET_MAX_CHARS must still produce a snippet
        // that contains the whole match, not silently drop the hit.
        let needle: String = "a".repeat(SNIPPET_MAX_CHARS + 40);
        let haystack = format!("prefix {needle} suffix");
        let s = build_snippet(&haystack, &needle).expect("should find long needle");
        assert!(s.contains(&needle));
    }

    #[test]
    fn build_snippet_returns_none_for_missing_needle() {
        assert!(build_snippet("hello world", "missing").is_none());
    }

    #[test]
    fn build_snippet_returns_none_for_empty_needle() {
        assert!(build_snippet("hello", "").is_none());
    }

    #[test]
    fn build_snippet_cancels_inside_one_large_missing_message() {
        let haystack = "a".repeat(2 * 1024 * 1024);
        let checks = AtomicUsize::new(0);
        let is_cancelled = || checks.fetch_add(1, Ordering::Relaxed) >= 4;

        let result = build_snippet_cancellable(&haystack, "not-present", &is_cancelled);

        assert_eq!(result, Err(SnippetSearchCancelled));
        assert!(
            checks.load(Ordering::Relaxed) < 10,
            "a detached search should stop after a few fixed-size chunks"
        );
    }

    #[test]
    fn sqlite_interrupt_stops_one_long_step_before_row_boundary() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory sqlite");
        let started = std::time::Instant::now();
        let result: rusqlite::Result<i64> = with_sqlite_cancellation(
            &conn,
            &|| started.elapsed() >= std::time::Duration::from_millis(2),
            || {
                conn.query_row(
                    "WITH RECURSIVE n(x) AS (
                         SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 100000000
                     ) SELECT sum(x) FROM n",
                    [],
                    |row| row.get(0),
                )
            },
        );

        assert!(result.is_err(), "the long sqlite step must be interrupted");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "cancellation must not wait for the recursive query to finish"
        );
    }
}
