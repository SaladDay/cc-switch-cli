use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tempfile::TempDir;

use crate::codex_config::{get_codex_config_dir, read_codex_config_text};
use crate::codex_state_db::codex_state_db_paths;
use crate::session_manager::cache::{self, FileScanTarget};
use crate::session_manager::paged_manifest::IdentityRowEnricher;
use crate::session_manager::scan_cache_store::ScanCacheStore;
use crate::session_manager::{
    SearchSnippet, SessionMessage, SessionMessageBatch, SessionMessageBatchBuilder, SessionMeta,
    SessionSearchHit,
};

use super::utils::{
    build_snippet_cancellable, extract_text, file_modified_ms, parse_timestamp_to_ms,
    path_basename, read_head_tail_lines_bounded, truncate_summary, visit_bounded_lines_cancellable,
    visit_bounded_lines_cancellable_with_status, with_sqlite_cancellation, TITLE_MAX_CHARS,
};

const PROVIDER_ID: &str = "codex";
const CODEX_SESSION_INDEX_FILENAME: &str = "session_index.jsonl";
#[cfg(not(test))]
// Title candidates are much smaller than SessionMeta rows. A wider spill
// amortizes temporary-file creation for large histories while remaining
// bounded to a few MiB even when every accepted ID reaches its byte ceiling.
const TITLE_RUN_SPILL_ROWS: usize = 4_096;
#[cfg(test)]
const TITLE_RUN_SPILL_ROWS: usize = 8;
#[cfg(not(test))]
const TITLE_RUN_MERGE_FAN_IN: usize = 8;
#[cfg(test)]
const TITLE_RUN_MERGE_FAN_IN: usize = 3;
const TITLE_RUN_SESSION_ID_MAX_BYTES: usize = 1024;

static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
        .unwrap()
});

#[derive(Deserialize)]
struct SessionIndexEntry {
    id: String,
    thread_name: String,
}

/// A bounded, sortable title candidate. Higher source ranks and later source
/// positions win within one session. Runs are ordered by session ID first so
/// the final enricher can advance alongside the identity-ordered manifest.
#[derive(Debug, Deserialize, Serialize)]
struct TitleCandidate {
    session_id: String,
    source_rank: u32,
    sequence: u64,
    title: String,
}

fn compare_title_candidates(left: &TitleCandidate, right: &TitleCandidate) -> Ordering {
    left.session_id
        .cmp(&right.session_id)
        .then_with(|| right.source_rank.cmp(&left.source_rank))
        .then_with(|| right.sequence.cmp(&left.sequence))
        .then_with(|| left.title.cmp(&right.title))
}

#[derive(Debug)]
enum TitleRunBuildError {
    Cancelled,
    Io(io::Error),
}

impl From<io::Error> for TitleRunBuildError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

struct TitleRunBuilder<'a> {
    workspace: TempDir,
    buffer: Vec<TitleCandidate>,
    run_levels: Vec<Vec<PathBuf>>,
    next_run_id: usize,
    is_cancelled: &'a (dyn Fn() -> bool + Sync),
    #[cfg(test)]
    peak_buffer_rows: usize,
    #[cfg(test)]
    merge_count: usize,
}

impl<'a> TitleRunBuilder<'a> {
    fn new(is_cancelled: &'a (dyn Fn() -> bool + Sync)) -> Result<Self, TitleRunBuildError> {
        if is_cancelled() {
            return Err(TitleRunBuildError::Cancelled);
        }
        let workspace = tempfile::Builder::new()
            .prefix("cc-switch-codex-titles-")
            .tempdir()?;
        Ok(Self {
            workspace,
            buffer: Vec::with_capacity(TITLE_RUN_SPILL_ROWS),
            run_levels: Vec::new(),
            next_run_id: 0,
            is_cancelled,
            #[cfg(test)]
            peak_buffer_rows: 0,
            #[cfg(test)]
            merge_count: 0,
        })
    }

    fn push(&mut self, candidate: TitleCandidate) -> Result<(), TitleRunBuildError> {
        if (self.is_cancelled)() {
            return Err(TitleRunBuildError::Cancelled);
        }
        if candidate.session_id.is_empty()
            || candidate.session_id.len() > TITLE_RUN_SESSION_ID_MAX_BYTES
            || candidate.title.is_empty()
        {
            return Ok(());
        }
        self.buffer.push(candidate);
        #[cfg(test)]
        {
            self.peak_buffer_rows = self.peak_buffer_rows.max(self.buffer.len());
        }
        if self.buffer.len() >= TITLE_RUN_SPILL_ROWS {
            self.flush_buffer()?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Option<CodexTitleEnricher>, TitleRunBuildError> {
        self.flush_buffer()?;
        if (self.is_cancelled)() {
            return Err(TitleRunBuildError::Cancelled);
        }

        // Compaction keeps at most fan-in minus one runs at each level while
        // ingesting. Only that logarithmic remainder is flattened here, then
        // reduced in fixed-width groups until one deduplicated run remains.
        let levels = std::mem::take(&mut self.run_levels);
        let mut runs = levels.into_iter().flatten().collect::<Vec<_>>();
        if runs.is_empty() {
            return Ok(None);
        }
        while runs.len() > 1 {
            let mut next = Vec::with_capacity(runs.len().div_ceil(TITLE_RUN_MERGE_FAN_IN));
            let mut iter = runs.into_iter();
            loop {
                let group = iter
                    .by_ref()
                    .take(TITLE_RUN_MERGE_FAN_IN)
                    .collect::<Vec<_>>();
                if group.is_empty() {
                    break;
                }
                if group.len() == 1 {
                    next.push(group.into_iter().next().expect("one title run"));
                } else {
                    let merged = self.merge_runs(&group)?;
                    remove_title_runs(&group);
                    next.push(merged);
                }
            }
            runs = next;
        }

        let final_path = runs.pop().expect("non-empty title runs");
        let reader = BufReader::new(File::open(&final_path)?);
        let workspace = self.workspace;
        let mut enricher = CodexTitleEnricher {
            // Keep the reader before the workspace so its file handle closes
            // before TempDir recursively removes the run directory.
            reader: Some(reader),
            next: None,
            active: None,
            _workspace: Some(workspace),
            #[cfg(test)]
            peak_buffer_rows: self.peak_buffer_rows,
            #[cfg(test)]
            merge_count: self.merge_count,
        };
        enricher.advance().map_err(TitleRunBuildError::from)?;
        Ok(Some(enricher))
    }

    fn flush_buffer(&mut self) -> Result<(), TitleRunBuildError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        if (self.is_cancelled)() {
            return Err(TitleRunBuildError::Cancelled);
        }

        let mut candidates = Vec::with_capacity(TITLE_RUN_SPILL_ROWS);
        std::mem::swap(&mut candidates, &mut self.buffer);
        candidates.sort_unstable_by(compare_title_candidates);
        candidates.dedup_by(|left, right| left.session_id == right.session_id);
        let path = self.next_run_path("spill");
        write_title_run(&path, &candidates)?;
        self.add_run(0, path)
    }

    fn add_run(&mut self, mut level: usize, mut path: PathBuf) -> Result<(), TitleRunBuildError> {
        loop {
            if self.run_levels.len() <= level {
                self.run_levels.resize_with(level + 1, Vec::new);
            }
            self.run_levels[level].push(path);
            if self.run_levels[level].len() < TITLE_RUN_MERGE_FAN_IN {
                return Ok(());
            }

            let inputs = std::mem::take(&mut self.run_levels[level]);
            path = self.merge_runs(&inputs)?;
            remove_title_runs(&inputs);
            level += 1;
        }
    }

    fn merge_runs(&mut self, inputs: &[PathBuf]) -> Result<PathBuf, TitleRunBuildError> {
        if (self.is_cancelled)() {
            return Err(TitleRunBuildError::Cancelled);
        }
        debug_assert!(!inputs.is_empty() && inputs.len() <= TITLE_RUN_MERGE_FAN_IN);
        let output = self.next_run_path("merge");
        merge_title_runs(inputs, &output, self.is_cancelled)?;
        #[cfg(test)]
        {
            self.merge_count += 1;
        }
        Ok(output)
    }

    fn next_run_path(&mut self, kind: &str) -> PathBuf {
        let id = self.next_run_id;
        self.next_run_id = self.next_run_id.saturating_add(1);
        self.workspace.path().join(format!("{kind}-{id}.jsonl"))
    }
}

fn write_title_run(path: &Path, candidates: &[TitleCandidate]) -> io::Result<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    for candidate in candidates {
        write_title_candidate(&mut writer, candidate)?;
    }
    writer.flush()
}

fn write_title_candidate(
    writer: &mut BufWriter<File>,
    candidate: &TitleCandidate,
) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, candidate).map_err(io::Error::other)?;
    writer.write_all(b"\n")
}

fn read_title_candidate(reader: &mut BufReader<File>) -> io::Result<Option<TitleCandidate>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    serde_json::from_str(line.trim_end())
        .map(Some)
        .map_err(io::Error::other)
}

fn merge_title_runs(
    inputs: &[PathBuf],
    output: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), TitleRunBuildError> {
    struct RunCursor {
        reader: BufReader<File>,
        next: Option<TitleCandidate>,
    }

    let mut cursors = Vec::with_capacity(inputs.len());
    for path in inputs {
        let mut reader = BufReader::new(File::open(path)?);
        let next = read_title_candidate(&mut reader)?;
        cursors.push(RunCursor { reader, next });
    }

    let mut writer = BufWriter::new(File::create(output)?);
    let mut last_session_id = None::<String>;
    loop {
        if is_cancelled() {
            return Err(TitleRunBuildError::Cancelled);
        }
        let Some(index) = cursors
            .iter()
            .enumerate()
            .filter_map(|(index, cursor)| cursor.next.as_ref().map(|next| (index, next)))
            .min_by(|(_, left), (_, right)| compare_title_candidates(left, right))
            .map(|(index, _)| index)
        else {
            break;
        };

        let candidate = cursors[index]
            .next
            .take()
            .expect("selected title candidate");
        cursors[index].next = read_title_candidate(&mut cursors[index].reader)?;
        if last_session_id.as_deref() != Some(candidate.session_id.as_str()) {
            last_session_id = Some(candidate.session_id.clone());
            write_title_candidate(&mut writer, &candidate)?;
        }
    }
    writer.flush()?;
    Ok(())
}

fn remove_title_runs(paths: &[PathBuf]) {
    for path in paths {
        if let Err(error) = std::fs::remove_file(path) {
            log::debug!(
                "Failed to remove temporary Codex title run {}: {error}",
                path.display()
            );
        }
    }
}

pub(crate) struct CodexTitleEnricher {
    reader: Option<BufReader<File>>,
    next: Option<TitleCandidate>,
    active: Option<(String, String)>,
    _workspace: Option<TempDir>,
    #[cfg(test)]
    peak_buffer_rows: usize,
    #[cfg(test)]
    merge_count: usize,
}

impl CodexTitleEnricher {
    fn advance(&mut self) -> io::Result<()> {
        self.next = match self.reader.as_mut() {
            Some(reader) => read_title_candidate(reader)?,
            None => None,
        };
        if self.next.is_none() {
            self.reader = None;
        }
        Ok(())
    }

    fn advance_or_disable(&mut self) {
        if let Err(error) = self.advance() {
            log::warn!("Failed to read temporary Codex title run: {error}");
            self.next = None;
            self.reader = None;
        }
    }

    #[cfg(test)]
    fn workspace_path(&self) -> &Path {
        self._workspace.as_ref().expect("title workspace").path()
    }
}

impl IdentityRowEnricher for CodexTitleEnricher {
    fn enrich(&mut self, row: &mut SessionMeta) {
        if row.provider_id != PROVIDER_ID {
            return;
        }
        if let Some((session_id, title)) = &self.active {
            match session_id.as_str().cmp(row.session_id.as_str()) {
                Ordering::Equal => {
                    row.title = Some(title.clone());
                    return;
                }
                Ordering::Less => self.active = None,
                Ordering::Greater => return,
            }
        }

        while self
            .next
            .as_ref()
            .is_some_and(|candidate| candidate.session_id < row.session_id)
        {
            self.advance_or_disable();
        }
        let Some(candidate) = self.next.take() else {
            return;
        };
        if candidate.session_id != row.session_id {
            self.next = Some(candidate);
            return;
        }

        row.title = Some(candidate.title.clone());
        self.active = Some((candidate.session_id, candidate.title));
        self.advance_or_disable();
    }
}

fn load_thread_titles() -> HashMap<String, String> {
    load_thread_titles_cancellable(&|| false).unwrap_or_default()
}

fn load_thread_titles_cancellable(
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<HashMap<String, String>> {
    let config_dir = get_codex_config_dir();
    load_thread_titles_for_config_dir_cancellable(&config_dir, is_cancelled)
}

fn load_thread_titles_for_config_dir_cancellable(
    config_dir: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<HashMap<String, String>> {
    if is_cancelled() {
        return None;
    }
    let config_text = read_codex_config_text().unwrap_or_default();
    let db_paths = codex_state_db_paths(config_dir, &config_text);
    load_thread_titles_from_paths(
        &config_dir.join(CODEX_SESSION_INDEX_FILENAME),
        &db_paths,
        is_cancelled,
    )
}

fn load_thread_titles_from_paths(
    session_index_path: &Path,
    db_paths: &[PathBuf],
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<HashMap<String, String>> {
    let mut titles = load_thread_titles_from_session_index(session_index_path, is_cancelled)?;
    for db_path in db_paths {
        if is_cancelled() {
            return None;
        }
        // Match Codex itself: an explicit state DB title overrides the legacy
        // session-index fallback. A configured SQLite home is visited last.
        titles.extend(load_thread_titles_from_db(db_path, is_cancelled)?);
    }
    Some(titles)
}

fn load_thread_titles_from_session_index(
    index_path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<HashMap<String, String>> {
    if !index_path.exists() {
        return Some(HashMap::new());
    }

    let mut titles = HashMap::new();
    let result = visit_bounded_lines_cancellable(index_path, is_cancelled, &mut |line| {
        let Ok(entry) = serde_json::from_str::<SessionIndexEntry>(line.trim()) else {
            return ControlFlow::Continue(());
        };
        let id = entry.id.trim();
        let title = entry.thread_name.trim();
        if !id.is_empty() && !title.is_empty() {
            titles.insert(id.to_string(), truncate_summary(title, TITLE_MAX_CHARS));
        }
        ControlFlow::Continue(())
    });

    match result {
        Ok(Some(())) => Some(titles),
        Ok(None) => None,
        Err(_) if is_cancelled() => None,
        Err(err) => {
            log::warn!(
                "Failed to read Codex session index {}: {err}",
                index_path.display()
            );
            Some(titles)
        }
    }
}

fn load_thread_titles_from_db(
    db_path: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<HashMap<String, String>> {
    if is_cancelled() {
        return None;
    }
    if !db_path.exists() {
        return Some(HashMap::new());
    }

    let conn = match Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(conn) => conn,
        Err(err) => {
            log::warn!(
                "Failed to open Codex state database {}: {err}",
                db_path.display()
            );
            return Some(HashMap::new());
        }
    };
    if let Err(err) = conn.busy_timeout(Duration::from_secs(2)) {
        log::warn!(
            "Failed to set Codex state database busy timeout for {}: {err}",
            db_path.display()
        );
        return Some(HashMap::new());
    }

    // Mirror Codex's `distinct_thread_metadata_title`: the state DB wins only
    // for a non-empty title that differs from the first user message. Keep the
    // comparison in SQLite so a potentially huge first message never enters
    // Rust memory.
    let result = with_sqlite_cancellation(&conn, is_cancelled, || {
        let mut stmt = conn.prepare(
            "SELECT id, title FROM threads \
             WHERE title <> '' \
             AND (first_user_message IS NULL OR TRIM(title) <> TRIM(first_user_message))",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            Ok((id, title))
        })?;

        let mut titles = HashMap::new();
        for row in rows {
            if is_cancelled() {
                break;
            }
            let Ok((id, title)) = row else {
                continue;
            };
            let id = id.trim();
            let title = title.trim();
            if !id.is_empty() && !title.is_empty() {
                titles.insert(id.to_string(), truncate_summary(title, TITLE_MAX_CHARS));
            }
        }
        Ok::<_, rusqlite::Error>(titles)
    });

    if is_cancelled() {
        return None;
    }
    match result {
        Ok(titles) => Some(titles),
        Err(err) => {
            log::warn!(
                "Failed to query Codex thread titles from {}: {err}",
                db_path.display()
            );
            Some(HashMap::new())
        }
    }
}

fn build_bounded_title_enricher(
    session_index_path: &Path,
    db_paths: &[PathBuf],
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<CodexTitleEnricher>, cache::StreamScanStop> {
    match try_build_bounded_title_enricher(session_index_path, db_paths, is_cancelled) {
        Ok(enricher) => Ok(enricher),
        Err(TitleRunBuildError::Cancelled) => Err(cache::StreamScanStop::Cancelled),
        Err(TitleRunBuildError::Io(error)) => {
            // Titles are optional metadata. The authoritative JSONL rows have
            // already streamed successfully, so a temporary-disk failure must
            // not turn a complete scan into an incomplete generation.
            log::warn!("Failed to build bounded Codex title overlay: {error}");
            Ok(None)
        }
    }
}

fn try_build_bounded_title_enricher(
    session_index_path: &Path,
    db_paths: &[PathBuf],
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<CodexTitleEnricher>, TitleRunBuildError> {
    let mut builder = TitleRunBuilder::new(is_cancelled)?;
    append_session_index_title_candidates(session_index_path, &mut builder, is_cancelled)?;
    for (index, db_path) in db_paths.iter().enumerate() {
        if is_cancelled() {
            return Err(TitleRunBuildError::Cancelled);
        }
        // The paths are ordered default first and configured override last.
        // Give later databases a larger rank to preserve that precedence.
        let source_rank = u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX);
        append_db_title_candidates(db_path, source_rank, &mut builder, is_cancelled)?;
    }
    builder.finish()
}

fn append_session_index_title_candidates(
    index_path: &Path,
    builder: &mut TitleRunBuilder<'_>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), TitleRunBuildError> {
    if !index_path.exists() {
        return Ok(());
    }

    let mut sequence = 0_u64;
    let mut build_error = None;
    let result = visit_bounded_lines_cancellable(index_path, is_cancelled, &mut |line| {
        let current_sequence = sequence;
        sequence = sequence.saturating_add(1);
        let Ok(entry) = serde_json::from_str::<SessionIndexEntry>(line.trim()) else {
            return ControlFlow::Continue(());
        };
        let session_id = entry.id.trim();
        let title = entry.thread_name.trim();
        if session_id.is_empty() || title.is_empty() {
            return ControlFlow::Continue(());
        }
        let candidate = TitleCandidate {
            session_id: session_id.to_string(),
            source_rank: 0,
            sequence: current_sequence,
            title: truncate_summary(title, TITLE_MAX_CHARS),
        };
        match builder.push(candidate) {
            Ok(()) => ControlFlow::Continue(()),
            Err(error) => {
                build_error = Some(error);
                ControlFlow::Break(())
            }
        }
    });
    if let Some(error) = build_error {
        return Err(error);
    }
    match result {
        Ok(Some(())) => Ok(()),
        Ok(None) => Err(TitleRunBuildError::Cancelled),
        Err(_) if is_cancelled() => Err(TitleRunBuildError::Cancelled),
        Err(error) => {
            // Preserve any candidates read before a recoverable source error,
            // then continue to the higher-priority state databases.
            log::warn!(
                "Failed to read Codex session index {}: {error}",
                index_path.display()
            );
            Ok(())
        }
    }
}

fn append_db_title_candidates(
    db_path: &Path,
    source_rank: u32,
    builder: &mut TitleRunBuilder<'_>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), TitleRunBuildError> {
    if is_cancelled() {
        return Err(TitleRunBuildError::Cancelled);
    }
    if !db_path.exists() {
        return Ok(());
    }

    let conn = match Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(conn) => conn,
        Err(error) => {
            log::warn!(
                "Failed to open Codex state database {}: {error}",
                db_path.display()
            );
            return Ok(());
        }
    };
    if let Err(error) = conn.busy_timeout(Duration::from_secs(2)) {
        log::warn!(
            "Failed to set Codex state database busy timeout for {}: {error}",
            db_path.display()
        );
        return Ok(());
    }

    let mut build_error = None;
    let result = with_sqlite_cancellation(&conn, is_cancelled, || {
        // Keep the same explicit-title predicate as Codex. The selected title
        // is trimmed and clipped in SQLite so one hostile DB cell cannot defeat
        // the per-record memory bound before Rust truncates it for display.
        let mut stmt = conn.prepare(
            "SELECT id, substr(TRIM(title), 1, ?1) FROM threads \
             WHERE title <> '' \
             AND (first_user_message IS NULL OR TRIM(title) <> TRIM(first_user_message)) \
             AND length(CAST(id AS BLOB)) <= ?2",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![
                i64::try_from(TITLE_MAX_CHARS.saturating_add(1)).unwrap_or(i64::MAX),
                i64::try_from(TITLE_RUN_SESSION_ID_MAX_BYTES).unwrap_or(i64::MAX)
            ],
            |row| {
                let id: String = row.get(0)?;
                let title: String = row.get(1)?;
                Ok((id, title))
            },
        )?;

        for row in rows {
            if is_cancelled() {
                break;
            }
            let Ok((id, title)) = row else {
                continue;
            };
            let session_id = id.trim();
            let title = title.trim();
            if session_id.is_empty() || title.is_empty() {
                continue;
            }
            let candidate = TitleCandidate {
                session_id: session_id.to_string(),
                source_rank,
                sequence: 0,
                title: truncate_summary(title, TITLE_MAX_CHARS),
            };
            if let Err(error) = builder.push(candidate) {
                build_error = Some(error);
                break;
            }
        }
        Ok::<_, rusqlite::Error>(())
    });
    if let Some(error) = build_error {
        return Err(error);
    }
    if is_cancelled() {
        return Err(TitleRunBuildError::Cancelled);
    }
    if let Err(error) = result {
        log::warn!(
            "Failed to query Codex thread titles from {}: {error}",
            db_path.display()
        );
    }
    Ok(())
}

fn overlay_thread_title(meta: &mut SessionMeta, thread_titles: &HashMap<String, String>) {
    if let Some(title) = thread_titles.get(&meta.session_id) {
        meta.title = Some(title.clone());
    }
}

fn overlay_thread_titles(sessions: &mut [SessionMeta], thread_titles: &HashMap<String, String>) {
    for session in sessions {
        overlay_thread_title(session, thread_titles);
    }
}

pub fn scan_sessions() -> Vec<SessionMeta> {
    let config_dir = get_codex_config_dir();
    let mut files = Vec::new();

    // 扫描活跃会话目录（按日期分区）
    let sessions_root = config_dir.join("sessions");
    collect_jsonl_files(&sessions_root, &mut files);

    // 扫描归档会话目录（扁平结构）
    let archived_root = config_dir.join("archived_sessions");
    collect_jsonl_files(&archived_root, &mut files);

    let mut sessions = super::utils::parse_sessions_parallel(files, parse_session);
    overlay_thread_titles(&mut sessions, &load_thread_titles());
    sessions
}

/// Cache-aware scan across the active and archived session directories.
pub(crate) fn scan_sessions_cached(store: &ScanCacheStore, force: bool) -> Vec<SessionMeta> {
    let mut sessions = cache::scan_provider_cached(
        store,
        PROVIDER_ID,
        scan_targets(),
        force,
        parse_session,
        |_| true,
    );
    overlay_thread_titles(&mut sessions, &load_thread_titles());
    sessions
}

pub(crate) fn scan_sessions_progressive(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(&SessionMeta),
) -> Vec<SessionMeta> {
    let thread_titles = load_thread_titles();
    let targets = scan_targets();
    let mut emit_enriched = |meta: &SessionMeta| {
        let mut enriched = meta.clone();
        overlay_thread_title(&mut enriched, &thread_titles);
        on_session(&enriched);
    };
    let mut sessions = match store {
        Some(store) => cache::scan_provider_cached_progressive(
            store,
            PROVIDER_ID,
            targets,
            force,
            parse_session,
            |_| true,
            &mut emit_enriched,
        ),
        None => {
            cache::scan_provider_uncached_progressive(targets, parse_session, &mut emit_enriched)
        }
    };
    overlay_thread_titles(&mut sessions, &thread_titles);
    sessions
}

pub(crate) fn scan_sessions_progressive_cancellable(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(&SessionMeta),
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<SessionMeta>> {
    let thread_titles = load_thread_titles_cancellable(is_cancelled)?;
    let targets = scan_targets_cancellable(is_cancelled)?;
    let mut emit_enriched = |meta: &SessionMeta| {
        let mut enriched = meta.clone();
        overlay_thread_title(&mut enriched, &thread_titles);
        on_session(&enriched);
    };
    let mut sessions = match store {
        Some(store) => cache::scan_provider_cached_progressive_cancellable(
            store,
            PROVIDER_ID,
            targets,
            force,
            parse_session,
            |_| true,
            &mut emit_enriched,
            is_cancelled,
        ),
        None => cache::scan_provider_uncached_progressive_cancellable(
            targets,
            parse_session,
            &mut emit_enriched,
            is_cancelled,
        ),
    }?;
    overlay_thread_titles(&mut sessions, &thread_titles);
    Some(sessions)
}

pub(crate) fn stream_sessions_cancellable(
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(SessionMeta) -> ControlFlow<()>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(cache::StreamScanStats, Option<Box<dyn IdentityRowEnricher>>), cache::StreamScanStop> {
    let config_dir = get_codex_config_dir();
    // Stream and cache only the JSONL-derived base rows. A rename can change
    // independently of a rollout fingerprint, so persisting it in the generic
    // cache would leave stale titles behind.
    let stats = stream_sessions_in_config_dir_cancellable(
        &config_dir,
        store,
        force,
        on_session,
        is_cancelled,
    )?;
    if stats.emitted == 0 {
        return Ok((stats, None));
    }
    if is_cancelled() {
        return Err(cache::StreamScanStop::Cancelled);
    }

    // Build the optional overlay only after every base row has left the
    // provider stream. This keeps time-to-first-row independent of the size of
    // session_index.jsonl and the Codex state database.
    let config_text = read_codex_config_text().unwrap_or_default();
    let db_paths = codex_state_db_paths(&config_dir, &config_text);
    let enricher = build_bounded_title_enricher(
        &config_dir.join(CODEX_SESSION_INDEX_FILENAME),
        &db_paths,
        is_cancelled,
    )?
    .map(|enricher| Box::new(enricher) as Box<dyn IdentityRowEnricher>);
    Ok((stats, enricher))
}

fn stream_sessions_in_config_dir_cancellable(
    config_dir: &Path,
    store: Option<&ScanCacheStore>,
    force: bool,
    on_session: &mut dyn FnMut(SessionMeta) -> ControlFlow<()>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<cache::StreamScanStats, cache::StreamScanStop> {
    cache::stream_file_provider_cancellable(
        store,
        PROVIDER_ID,
        force,
        |path| {
            if is_cancelled() {
                return Err(cache::StreamScanStop::Cancelled);
            }
            let meta = parse_session_authoritative(path)?;
            if is_cancelled() {
                Err(cache::StreamScanStop::Cancelled)
            } else {
                Ok(meta)
            }
        },
        |_| true,
        cache::stat_target,
        move |on_target, cancel| {
            cache::visit_targets_recursive_cancellable(
                &config_dir.join("sessions"),
                "jsonl",
                on_target,
                cancel,
            )?;
            cache::visit_targets_recursive_cancellable(
                &config_dir.join("archived_sessions"),
                "jsonl",
                on_target,
                cancel,
            )
        },
        on_session,
        is_cancelled,
    )
}

fn scan_targets() -> Vec<FileScanTarget> {
    scan_targets_cancellable(&|| false).expect("non-cancellable target scan cannot stop")
}

fn scan_targets_cancellable(
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<FileScanTarget>> {
    let config_dir = get_codex_config_dir();
    let mut targets = Vec::new();
    if !cache::collect_targets_recursive_cancellable(
        &config_dir.join("sessions"),
        "jsonl",
        &mut targets,
        is_cancelled,
    ) || !cache::collect_targets_recursive_cancellable(
        &config_dir.join("archived_sessions"),
        "jsonl",
        &mut targets,
        is_cancelled,
    ) {
        return None;
    }
    Some(targets)
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

        if value.get("type").and_then(Value::as_str) != Some("response_item") {
            return ControlFlow::Continue(());
        }

        let payload = match value.get("payload") {
            Some(payload) => payload,
            None => return ControlFlow::Continue(()),
        };

        let payload_type = payload.get("type").and_then(Value::as_str).unwrap_or("");

        // Codex uses separate payload types for tool interactions
        let (role, content) = match payload_type {
            "message" => {
                let role = payload
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                let content = payload.get("content").map(extract_text).unwrap_or_default();
                (role, content)
            }
            "function_call" => {
                let name = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                ("assistant".to_string(), format!("[Tool: {name}]"))
            }
            "function_call_output" => {
                let output = payload
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                ("tool".to_string(), output)
            }
            _ => return ControlFlow::Continue(()),
        };

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

/// Search a single Codex session file for `needle` (case-insensitive).
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
        if value.get("type").and_then(Value::as_str) != Some("response_item") {
            return ControlFlow::Continue(());
        }
        let payload = match value.get("payload") {
            Some(payload) => payload,
            None => return ControlFlow::Continue(()),
        };
        let payload_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
        let (role, content) = match payload_type {
            "message" => {
                let role = payload
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                let content = payload.get("content").map(extract_text).unwrap_or_default();
                (role, content)
            }
            "function_call" => {
                let name = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                ("assistant".to_string(), format!("[Tool: {name}]"))
            }
            "function_call_output" => {
                let output = payload
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                ("tool".to_string(), output)
            }
            _ => return ControlFlow::Continue(()),
        };
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
    let meta = parse_session(path)
        .ok_or_else(|| format!("Failed to parse Codex session metadata: {}", path.display()))?;

    if meta.session_id != session_id {
        return Err(format!(
            "Codex session ID mismatch: expected {session_id}, found {}",
            meta.session_id
        ));
    }

    std::fs::remove_file(path).map_err(|e| {
        format!(
            "Failed to delete Codex session file {}: {e}",
            path.display()
        )
    })?;

    Ok(true)
}

fn parse_session(path: &Path) -> Option<SessionMeta> {
    parse_session_authoritative(path).ok().flatten()
}

fn parse_session_authoritative(path: &Path) -> Result<Option<SessionMeta>, cache::StreamScanStop> {
    let (head, tail) = read_head_tail_lines_bounded(path, 10, 30).map_err(|error| {
        log::warn!(
            "authoritative Codex metadata read failed at {}: {error}",
            path.display()
        );
        cache::StreamScanStop::Incomplete
    })?;
    Ok(parse_session_lines(path, &head, &tail))
}

fn parse_session_lines(path: &Path, head: &[String], tail: &[String]) -> Option<SessionMeta> {
    let mut session_id: Option<String> = None;
    let mut project_dir: Option<String> = None;
    let mut created_at: Option<i64> = None;
    let mut first_user_message: Option<String> = None;

    // Extract metadata and first user message from head lines
    for line in head {
        let value: Value = match serde_json::from_str(line) {
            Ok(parsed) => parsed,
            Err(_) => continue,
        };
        if created_at.is_none() {
            created_at = value.get("timestamp").and_then(parse_timestamp_to_ms);
        }
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            if let Some(payload) = value.get("payload") {
                if is_subagent_source(payload.get("source")) {
                    return None;
                }
                if session_id.is_none() {
                    session_id = payload
                        .get("id")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string());
                }
                if project_dir.is_none() {
                    project_dir = payload
                        .get("cwd")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string());
                }
                if let Some(ts) = payload.get("timestamp").and_then(parse_timestamp_to_ms) {
                    created_at.get_or_insert(ts);
                }
            }
        }
        // Extract first user message as title candidate
        if first_user_message.is_none()
            && value.get("type").and_then(Value::as_str) == Some("response_item")
        {
            if let Some(payload) = value.get("payload") {
                if payload.get("type").and_then(Value::as_str) == Some("message")
                    && payload.get("role").and_then(Value::as_str) == Some("user")
                {
                    let text = payload.get("content").map(extract_text).unwrap_or_default();
                    let trimmed = text.trim();
                    if !trimmed.is_empty()
                        && !trimmed.starts_with("# AGENTS.md")
                        && !trimmed.starts_with("<environment_context>")
                    {
                        first_user_message = Some(trimmed.to_string());
                    }
                }
            }
        }
        if session_id.is_some()
            && project_dir.is_some()
            && created_at.is_some()
            && first_user_message.is_some()
        {
            break;
        }
    }

    // Extract last_active_at and summary from tail lines (reverse order)
    let mut last_active_at: Option<i64> = None;
    let mut summary: Option<String> = None;

    for line in tail.iter().rev() {
        let value: Value = match serde_json::from_str(line) {
            Ok(parsed) => parsed,
            Err(_) => continue,
        };
        if last_active_at.is_none() {
            last_active_at = value.get("timestamp").and_then(parse_timestamp_to_ms);
        }
        if summary.is_none() && value.get("type").and_then(Value::as_str) == Some("response_item") {
            if let Some(payload) = value.get("payload") {
                if payload.get("type").and_then(Value::as_str) == Some("message") {
                    let text = payload.get("content").map(extract_text).unwrap_or_default();
                    if !text.trim().is_empty() {
                        summary = Some(text);
                    }
                }
            }
        }
        if last_active_at.is_some() && summary.is_some() {
            break;
        }
    }

    let session_id = session_id.or_else(|| infer_session_id_from_filename(path));
    let session_id = session_id?;

    let title = first_user_message
        .map(|t| truncate_summary(&t, TITLE_MAX_CHARS))
        .or_else(|| {
            project_dir
                .as_deref()
                .and_then(path_basename)
                .map(|v| v.to_string())
        })
        .or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .map(|value| truncate_summary(value, TITLE_MAX_CHARS))
        });

    let summary = summary.map(|text| truncate_summary(&text, 160));
    let fallback_time = file_modified_ms(path);

    Some(SessionMeta {
        provider_id: PROVIDER_ID.to_string(),
        session_id: session_id.clone(),
        title,
        summary,
        project_dir,
        created_at: created_at.or(fallback_time),
        last_active_at: last_active_at.or(fallback_time).or(created_at),
        source_path: Some(path.to_string_lossy().to_string()),
        resume_command: Some(format!("codex resume {session_id}")),
    })
}

fn is_subagent_source(source: Option<&Value>) -> bool {
    source
        .and_then(|value| value.as_object())
        .map(|source| source.contains_key("subagent"))
        .unwrap_or(false)
}

fn infer_session_id_from_filename(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_string_lossy();
    UUID_RE.find(&file_name).map(|mat| mat.as_str().to_string())
}

fn collect_jsonl_files(root: &Path, files: &mut Vec<PathBuf>) {
    if !root.exists() {
        return;
    }

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_state_db::CODEX_STATE_DB_FILENAME;
    use crate::session_manager::providers::utils::MAX_METADATA_LINE_BYTES;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    #[test]
    fn delete_session_removes_jsonl_file() {
        let temp = tempdir().expect("tempdir");
        let path = temp
            .path()
            .join("rollout-2026-03-06T21-50-12-019cc369-bd7c-7891-b371-7b20b4fe0b18.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-03-06T21:50:12Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"019cc369-bd7c-7891-b371-7b20b4fe0b18\",\"cwd\":\"/tmp/project\"}}\n",
                "{\"timestamp\":\"2026-03-06T21:50:13Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"hello\"}}\n"
            ),
        )
        .expect("write session");

        delete_session(temp.path(), &path, "019cc369-bd7c-7891-b371-7b20b4fe0b18")
            .expect("delete session");

        assert!(!path.exists());
    }

    #[test]
    fn parse_session_uses_first_user_message_as_title() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-03-06T21:50:12Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"test-id\",\"cwd\":\"/tmp/project\"}}\n",
                "{\"timestamp\":\"2026-03-06T21:50:13Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"How do I deploy?\"}}\n",
                "{\"timestamp\":\"2026-03-06T21:50:14Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"Here is how...\"}}\n"
            ),
        )
        .expect("write");

        let meta = parse_session(&path).unwrap();
        assert_eq!(meta.title.as_deref(), Some("How do I deploy?"));
    }

    #[test]
    fn state_db_titles_are_trimmed_and_only_keep_explicit_renames() {
        let temp = tempdir().expect("tempdir");
        let db_path = temp.path().join(CODEX_STATE_DB_FILENAME);
        let conn = Connection::open(&db_path).expect("open state db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                first_user_message TEXT
            );
            INSERT INTO threads (id, title, first_user_message) VALUES
                ('renamed', '  Renamed Codex thread  ', 'First prompt'),
                ('blank', '   ', 'First prompt'),
                ('same', '  First prompt  ', 'First prompt'),
                ('no-message', 'Name before first prompt', NULL);",
        )
        .expect("seed state db");
        drop(conn);

        let titles = load_thread_titles_from_db(&db_path, &|| false).expect("not cancelled");

        assert_eq!(
            titles.get("renamed").map(String::as_str),
            Some("Renamed Codex thread")
        );
        assert_eq!(
            titles.get("no-message").map(String::as_str),
            Some("Name before first prompt")
        );
        assert!(!titles.contains_key("blank"));
        assert!(!titles.contains_key("same"));
    }

    #[test]
    fn session_index_uses_latest_valid_bounded_name() {
        let temp = tempdir().expect("tempdir");
        let index_path = temp.path().join(CODEX_SESSION_INDEX_FILENAME);
        let oversized = "x".repeat(MAX_METADATA_LINE_BYTES + 1);
        std::fs::write(
            &index_path,
            format!(
                concat!(
                    "{{\"id\":\"thread-1\",\"thread_name\":\"Old name\"}}\n",
                    "{{\"id\":\"oversized\",\"thread_name\":\"{oversized}\"}}\n",
                    "not json\n",
                    "{{\"id\":\"thread-1\",\"thread_name\":\"  New name  \"}}\n"
                ),
                oversized = oversized,
            ),
        )
        .expect("write session index");

        let titles =
            load_thread_titles_from_session_index(&index_path, &|| false).expect("not cancelled");

        assert_eq!(titles.get("thread-1").map(String::as_str), Some("New name"));
        assert!(!titles.contains_key("oversized"));
    }

    #[test]
    fn state_db_explicit_title_overrides_session_index_fallback() {
        let temp = tempdir().expect("tempdir");
        let index_path = temp.path().join(CODEX_SESSION_INDEX_FILENAME);
        std::fs::write(
            &index_path,
            concat!(
                "{\"id\":\"thread-1\",\"thread_name\":\"Legacy name\"}\n",
                "{\"id\":\"thread-2\",\"thread_name\":\"Legacy fallback\"}\n"
            ),
        )
        .expect("write session index");

        let db_path = temp.path().join(CODEX_STATE_DB_FILENAME);
        let conn = Connection::open(&db_path).expect("open state db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                first_user_message TEXT
            );
            INSERT INTO threads (id, title, first_user_message) VALUES
                ('thread-1', 'SQLite name', 'First prompt'),
                ('thread-2', 'First prompt', 'First prompt');",
        )
        .expect("seed state db");
        drop(conn);

        let titles = load_thread_titles_from_paths(&index_path, &[db_path], &|| false)
            .expect("not cancelled");

        assert_eq!(
            titles.get("thread-1").map(String::as_str),
            Some("SQLite name")
        );
        assert_eq!(
            titles.get("thread-2").map(String::as_str),
            Some("Legacy fallback")
        );
    }

    #[test]
    fn streaming_keeps_renames_out_of_cache_and_enriches_after_base_rows() {
        let temp = tempdir().expect("tempdir");
        let sessions_dir = temp.path().join("sessions/2026/07/16");
        std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        let path = sessions_dir.join("rollout-test-id.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-07-16T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"test-id\",\"cwd\":\"/tmp/project\"}}\n",
                "{\"timestamp\":\"2026-07-16T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"First prompt\"}}\n"
            ),
        )
        .expect("write session");
        let store = ScanCacheStore::in_memory().expect("open cache");
        let index_path = temp.path().join(CODEX_SESSION_INDEX_FILENAME);

        let scan = |title: Option<&str>| {
            match title {
                Some(title) => std::fs::write(
                    &index_path,
                    format!("{{\"id\":\"test-id\",\"thread_name\":{title:?}}}\n"),
                )
                .expect("write title index"),
                None => {
                    let _ = std::fs::remove_file(&index_path);
                }
            }
            let mut emitted = Vec::new();
            let stats = stream_sessions_in_config_dir_cancellable(
                temp.path(),
                Some(&store),
                false,
                &mut |meta| {
                    emitted.push(meta);
                    ControlFlow::Continue(())
                },
                &|| false,
            )
            .expect("scan sessions");
            assert_eq!(emitted[0].title.as_deref(), Some("First prompt"));

            let mut enricher = build_bounded_title_enricher(&index_path, &[], &|| false)
                .expect("build title overlay");
            if let Some(enricher) = enricher.as_mut() {
                enricher.enrich(&mut emitted[0]);
            }
            (stats, emitted)
        };

        let (first_stats, first) = scan(Some("First renamed title"));
        assert_eq!(first_stats.reparsed, 1);
        assert_eq!(first[0].title.as_deref(), Some("First renamed title"));

        let (second_stats, second) = scan(Some("Second renamed title"));
        assert_eq!(second_stats.cache_hits, 1);
        assert_eq!(second[0].title.as_deref(), Some("Second renamed title"));

        let (third_stats, third) = scan(None);
        assert_eq!(third_stats.cache_hits, 1);
        assert_eq!(third[0].title.as_deref(), Some("First prompt"));
    }

    #[test]
    fn bounded_title_runs_merge_priority_latest_index_and_clean_up() {
        let temp = tempdir().expect("tempdir");
        let index_path = temp.path().join(CODEX_SESSION_INDEX_FILENAME);
        let mut index = String::from(
            "{\"id\":\"index-only\",\"thread_name\":\"Old index title\"}\n\
             {\"id\":\"thread-shared\",\"thread_name\":\"Index title\"}\n",
        );
        for number in 0..80 {
            index.push_str(&format!(
                "{{\"id\":\"bulk-{number:03}\",\"thread_name\":\"Bulk {number}\"}}\n"
            ));
        }
        index.push_str("{\"id\":\"index-only\",\"thread_name\":\"Latest index title\"}\n");
        std::fs::write(&index_path, index).expect("write index");

        let default_db = temp.path().join("default.sqlite");
        let conn = Connection::open(&default_db).expect("open default db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                first_user_message TEXT
            );
            INSERT INTO threads (id, title, first_user_message) VALUES
                ('thread-default', 'Default DB title', 'First prompt'),
                ('thread-shared', 'Default shared title', 'First prompt');",
        )
        .expect("seed default db");
        drop(conn);

        let configured_db = temp.path().join("configured.sqlite");
        let conn = Connection::open(&configured_db).expect("open configured db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                first_user_message TEXT
            );
            INSERT INTO threads (id, title, first_user_message) VALUES
                ('thread-shared', 'Configured DB title', 'First prompt');",
        )
        .expect("seed configured db");
        drop(conn);

        let mut enricher =
            try_build_bounded_title_enricher(&index_path, &[default_db, configured_db], &|| false)
                .expect("build title runs")
                .expect("title enricher");
        assert!(enricher.peak_buffer_rows <= TITLE_RUN_SPILL_ROWS);
        assert!(
            enricher.merge_count >= 2,
            "the fixture should exercise multi-stage fixed-fan-in merging"
        );
        let workspace = enricher.workspace_path().to_path_buf();
        assert!(workspace.exists());

        let mut index_only = SessionMeta {
            provider_id: PROVIDER_ID.to_string(),
            session_id: "index-only".to_string(),
            title: Some("JSONL title".to_string()),
            ..SessionMeta::default()
        };
        enricher.enrich(&mut index_only);
        assert_eq!(index_only.title.as_deref(), Some("Latest index title"));

        let mut default_only = SessionMeta {
            provider_id: PROVIDER_ID.to_string(),
            session_id: "thread-default".to_string(),
            title: Some("JSONL title".to_string()),
            ..SessionMeta::default()
        };
        enricher.enrich(&mut default_only);
        assert_eq!(default_only.title.as_deref(), Some("Default DB title"));

        for source in ["active.jsonl", "archived.jsonl"] {
            let mut shared = SessionMeta {
                provider_id: PROVIDER_ID.to_string(),
                session_id: "thread-shared".to_string(),
                title: Some("JSONL title".to_string()),
                source_path: Some(source.to_string()),
                ..SessionMeta::default()
            };
            enricher.enrich(&mut shared);
            assert_eq!(shared.title.as_deref(), Some("Configured DB title"));
        }

        drop(enricher);
        assert!(!workspace.exists(), "TempDir should remove all title runs");
    }

    #[test]
    fn parse_session_skips_agents_md_injection() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-03-06T21:50:12Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"test-id\",\"cwd\":\"/tmp/project\"}}\n",
                "{\"timestamp\":\"2026-03-06T21:50:13Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"developer\",\"content\":\"<permissions>\"}}\n",
                "{\"timestamp\":\"2026-03-06T21:50:13Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"# AGENTS.md instructions for /tmp/project\\n<INSTRUCTIONS>Do stuff</INSTRUCTIONS>\"}}\n",
                "{\"timestamp\":\"2026-03-06T21:50:14Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"Fix the login bug\"}}\n"
            ),
        )
        .expect("write");

        let meta = parse_session(&path).unwrap();
        // Should skip AGENTS.md injection and use the real user message
        assert_eq!(meta.title.as_deref(), Some("Fix the login bug"));
    }

    #[test]
    fn parse_session_skips_subagent_sessions() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-04-28T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"subagent-id\",\"cwd\":\"/tmp/project\",\"originator\":\"codex-tui\",\"source\":{\"subagent\":{\"thread_spawn\":{\"parent_thread_id\":\"parent-id\",\"depth\":1,\"agent_role\":\"explorer\"}}}}}\n",
                "{\"timestamp\":\"2026-04-28T10:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"Inspect the project\"}}\n"
            ),
        )
        .expect("write");

        assert!(parse_session(&path).is_none());
    }

    #[test]
    fn parse_session_skips_environment_context_injection() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-03-06T21:50:12Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"test-id\",\"cwd\":\"/tmp/project\"}}\n",
                "{\"timestamp\":\"2026-03-06T21:50:13Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"<environment_context>\\n  <cwd>/tmp/project</cwd>\\n</environment_context>\"}}\n",
                "{\"timestamp\":\"2026-03-06T21:50:14Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"Fix the login bug\"}}\n"
            ),
        )
        .expect("write");

        let meta = parse_session(&path).unwrap();
        // Should skip environment_context injection and use the real user message
        assert_eq!(meta.title.as_deref(), Some("Fix the login bug"));
    }

    #[test]
    fn parse_session_falls_back_to_dir_basename() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-03-06T21:50:12Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"test-id\",\"cwd\":\"/tmp/my-project\"}}\n",
                "{\"timestamp\":\"2026-03-06T21:50:13Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"Hello\"}}\n"
            ),
        )
        .expect("write");

        let meta = parse_session(&path).unwrap();
        // No user message → falls back to dir basename
        assert_eq!(meta.title.as_deref(), Some("my-project"));
    }

    #[test]
    fn parse_session_truncates_long_title() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl");
        let long_msg = "a".repeat(200);
        std::fs::write(
            &path,
            format!(
                "{{\"timestamp\":\"2026-03-06T21:50:12Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"test-id\",\"cwd\":\"/tmp/p\"}}}}\n\
                 {{\"timestamp\":\"2026-03-06T21:50:13Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"{long_msg}\"}}}}\n",
            ),
        )
        .expect("write");

        let meta = parse_session(&path).unwrap();
        let title = meta.title.unwrap();
        assert!(title.len() <= TITLE_MAX_CHARS + 3); // +3 for "..."
        assert!(title.ends_with("..."));
    }

    #[test]
    fn load_messages_includes_function_call_and_output() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-03-06T21:50:12Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"test-id\",\"cwd\":\"/tmp\"}}\n",
                "{\"timestamp\":\"2026-03-06T21:50:13Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"list files\"}}\n",
                "{\"timestamp\":\"2026-03-06T21:50:14Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"shell\",\"arguments\":\"{\\\"cmd\\\":[\\\"ls\\\"]}\",\"call_id\":\"call_1\"}}\n",
                "{\"timestamp\":\"2026-03-06T21:50:15Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"call_1\",\"output\":\"file1.txt\\nfile2.txt\"}}\n",
                "{\"timestamp\":\"2026-03-06T21:50:16Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Done.\"}]}}\n",
            ),
        )
        .expect("write");

        let msgs = load_messages(&path).expect("load");
        assert_eq!(msgs.len(), 4);

        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "list files");

        assert_eq!(msgs[1].role, "assistant");
        assert!(msgs[1].content.contains("[Tool: shell]"));

        assert_eq!(msgs[2].role, "tool");
        assert!(msgs[2].content.contains("file1.txt"));

        assert_eq!(msgs[3].role, "assistant");
        assert_eq!(msgs[3].content, "Done.");
    }

    #[test]
    fn load_messages_stops_reading_jsonl_at_the_preview_limit() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl");
        let mut transcript = String::new();
        for index in 0..1_000 {
            transcript.push_str(&format!(
                "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"message {index}\"}}}}\n"
            ));
        }
        std::fs::write(&path, transcript).expect("write");
        let cancellation_checks = AtomicUsize::new(0);

        let batch = load_messages_cancellable(&path, &|| {
            cancellation_checks.fetch_add(1, Ordering::AcqRel);
            false
        })
        .expect("load bounded preview");

        assert_eq!(
            batch.messages.len(),
            crate::session_manager::SESSION_MESSAGE_PREVIEW_MAX_MESSAGES
        );
        assert!(batch.truncated);
        assert!(
            cancellation_checks.load(Ordering::Acquire)
                <= crate::session_manager::SESSION_MESSAGE_PREVIEW_MAX_MESSAGES + 2,
            "the JSONL visitor must not scan the tail after the batch fills"
        );
    }

    #[test]
    fn load_messages_observes_cancellation_before_reading() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("session.jsonl");
        std::fs::write(&path, "{}\n").expect("write");

        let error = load_messages_cancellable(&path, &|| true).expect_err("cancelled load");

        assert!(error.contains("cancelled"));
    }
}
