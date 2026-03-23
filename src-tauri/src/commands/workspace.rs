use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::write_text_file;
use crate::openclaw_config::get_openclaw_dir;

pub const ALLOWED_FILES: &[&str] = &[
    "AGENTS.md",
    "SOUL.md",
    "USER.md",
    "IDENTITY.md",
    "TOOLS.md",
    "MEMORY.md",
    "HEARTBEAT.md",
    "BOOTSTRAP.md",
    "BOOT.md",
];

const PREVIEW_CHAR_LIMIT: usize = 120;
const SNIPPET_CONTEXT_CHARS: usize = 40;

#[doc(hidden)]
pub type AppHandle = ();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DailyMemoryFileInfo {
    pub filename: String,
    pub date: String,
    pub size_bytes: u64,
    pub modified_at: u64,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DailyMemorySearchResult {
    pub filename: String,
    pub date: String,
    pub size_bytes: u64,
    pub modified_at: u64,
    pub snippet: String,
    pub match_count: usize,
}

struct DailyMemoryEntry {
    filename: String,
    content: Option<String>,
    size_bytes: u64,
    modified_at: u64,
}

pub fn read_workspace_file(filename: String) -> Result<Option<String>, String> {
    validate_workspace_filename(&filename)?;
    ensure_workspace_root_safe()?;

    let path = workspace_dir().join(filename);
    if is_symlink(&path)? {
        return Err(format!(
            "Refusing to read workspace file symlink: {}",
            path.display()
        ));
    }
    if !path.exists() {
        return Ok(None);
    }

    fs::read_to_string(&path)
        .map(Some)
        .map_err(|error| format!("Failed to read workspace file {}: {error}", path.display()))
}

pub fn workspace_file_exists(filename: String) -> Result<bool, String> {
    validate_workspace_filename(&filename)?;
    ensure_workspace_root_safe()?;

    let path = workspace_dir().join(filename);
    if is_symlink(&path)? {
        return Ok(false);
    }

    Ok(path.exists())
}

pub fn write_workspace_file(filename: String, content: String) -> Result<(), String> {
    validate_workspace_filename(&filename)?;
    ensure_workspace_root_safe()?;

    let path = workspace_dir().join(filename);
    write_text_file(&path, &content)
        .map_err(|error| format!("Failed to write workspace file {}: {error}", path.display()))
}

pub fn open_workspace_directory(_handle: AppHandle, subdir: String) -> Result<bool, String> {
    open_workspace_directory_core(&subdir)
}

fn open_workspace_directory_core(subdir: &str) -> Result<bool, String> {
    let target_dir = if subdir == "memory" {
        ensure_daily_memory_root_safe()?;
        daily_memory_dir()
    } else {
        ensure_workspace_root_safe()?;
        workspace_dir()
    };

    fs::create_dir_all(&target_dir).map_err(|error| {
        format!(
            "Failed to create workspace directory {}: {error}",
            target_dir.display()
        )
    })?;

    if std::env::var_os("CC_SWITCH_TEST_DISABLE_OPEN").is_some() {
        return Ok(true);
    }

    open_directory(&target_dir)
}

pub fn list_daily_memory_files() -> Result<Vec<DailyMemoryFileInfo>, String> {
    let mut entries = read_daily_memory_entries()?;
    entries.sort_by(|left, right| right.filename.cmp(&left.filename));

    Ok(entries
        .into_iter()
        .map(|entry| DailyMemoryFileInfo {
            date: daily_memory_date(&entry.filename),
            filename: entry.filename,
            size_bytes: entry.size_bytes,
            modified_at: entry.modified_at,
            preview: entry
                .content
                .as_deref()
                .map(preview_text)
                .unwrap_or_default(),
        })
        .collect())
}

pub fn read_daily_memory_file(filename: String) -> Result<Option<String>, String> {
    validate_daily_memory_filename(&filename)?;
    ensure_daily_memory_root_safe()?;

    let path = daily_memory_dir().join(filename);
    if is_symlink(&path)? {
        return Err(format!(
            "Refusing to read daily memory symlink: {}",
            path.display()
        ));
    }
    if !path.exists() {
        return Ok(None);
    }

    fs::read_to_string(&path).map(Some).map_err(|error| {
        format!(
            "Failed to read daily memory file {}: {error}",
            path.display()
        )
    })
}

pub fn write_daily_memory_file(filename: String, content: String) -> Result<(), String> {
    validate_daily_memory_filename(&filename)?;
    ensure_daily_memory_root_safe()?;

    let path = daily_memory_dir().join(filename);
    write_text_file(&path, &content).map_err(|error| {
        format!(
            "Failed to write daily memory file {}: {error}",
            path.display()
        )
    })
}

pub fn search_daily_memory_files(query: String) -> Result<Vec<DailyMemorySearchResult>, String> {
    let trimmed_query = query.trim();
    if trimmed_query.is_empty() {
        return Ok(Vec::new());
    }

    let query_lower = trimmed_query.to_lowercase();
    let mut results = Vec::new();

    for entry in read_daily_memory_entries()? {
        let date = daily_memory_date(&entry.filename);
        let date_matches = date.to_lowercase().contains(&query_lower);
        let content_match_count = entry
            .content
            .as_deref()
            .map(|content| count_case_insensitive_matches(content, trimmed_query))
            .unwrap_or(0);

        if !date_matches && content_match_count == 0 {
            continue;
        }

        let snippet = entry
            .content
            .as_deref()
            .and_then(|content| {
                first_case_insensitive_match_range(content, trimmed_query)
                    .map(|(start, end)| snippet_text(content, start, end))
                    .or_else(|| date_matches.then(|| preview_text(content)))
            })
            .unwrap_or_default();

        results.push(DailyMemorySearchResult {
            filename: entry.filename,
            date,
            size_bytes: entry.size_bytes,
            modified_at: entry.modified_at,
            snippet,
            match_count: content_match_count,
        });
    }

    results.sort_by(|left, right| right.filename.cmp(&left.filename));
    Ok(results)
}

pub fn delete_daily_memory_file(filename: String) -> Result<(), String> {
    validate_daily_memory_filename(&filename)?;
    ensure_daily_memory_root_safe()?;

    let path = daily_memory_dir().join(filename);
    if !path.exists() {
        return Ok(());
    }

    fs::remove_file(&path).map_err(|error| {
        format!(
            "Failed to delete daily memory file {}: {error}",
            path.display()
        )
    })
}

fn validate_workspace_filename(filename: &str) -> Result<(), String> {
    if ALLOWED_FILES.contains(&filename) {
        Ok(())
    } else {
        Err(format!("Invalid workspace filename: {filename}"))
    }
}

fn validate_daily_memory_filename(filename: &str) -> Result<(), String> {
    if daily_memory_filename_regex().is_match(filename) {
        Ok(())
    } else {
        Err(format!("Invalid daily memory filename: {filename}"))
    }
}

fn workspace_dir() -> PathBuf {
    get_openclaw_dir().join("workspace")
}

fn daily_memory_dir() -> PathBuf {
    workspace_dir().join("memory")
}

fn daily_memory_filename_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^\d{4}-\d{2}-\d{2}\.md$").expect("valid regex"))
}

fn read_daily_memory_entries() -> Result<Vec<DailyMemoryEntry>, String> {
    ensure_daily_memory_root_safe()?;

    let dir = daily_memory_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    let read_dir = fs::read_dir(&dir).map_err(|error| {
        format!(
            "Failed to read daily memory directory {}: {error}",
            dir.display()
        )
    })?;

    for entry in read_dir {
        let entry = entry.map_err(|error| {
            format!(
                "Failed to read daily memory directory entry {}: {error}",
                dir.display()
            )
        })?;

        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_file() {
            continue;
        }

        let filename = entry.file_name().to_string_lossy().to_string();
        if validate_daily_memory_filename(&filename).is_err() {
            continue;
        }

        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let content = fs::read_to_string(&path).ok();

        entries.push(DailyMemoryEntry {
            filename,
            content,
            size_bytes: metadata.len(),
            modified_at: modified_at_seconds(&metadata),
        });
    }

    Ok(entries)
}

fn daily_memory_date(filename: &str) -> String {
    filename.trim_end_matches(".md").to_string()
}

fn modified_at_seconds(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn preview_text(content: &str) -> String {
    truncate_chars(content, PREVIEW_CHAR_LIMIT)
}

fn snippet_text(content: &str, match_start: usize, match_end: usize) -> String {
    let total_chars = content.chars().count();
    let match_start_chars = content[..match_start].chars().count();
    let match_end_chars = content[..match_end].chars().count();

    let snippet_start_chars = match_start_chars.saturating_sub(SNIPPET_CONTEXT_CHARS);
    let snippet_end_chars = (match_end_chars + SNIPPET_CONTEXT_CHARS).min(total_chars);

    let start_byte = byte_index_for_char(content, snippet_start_chars);
    let end_byte = byte_index_for_char(content, snippet_end_chars);
    let mut snippet = String::new();

    if snippet_start_chars > 0 {
        snippet.push_str("...");
    }
    snippet.push_str(&content[start_byte..end_byte]);
    if snippet_end_chars < total_chars {
        snippet.push_str("...");
    }

    snippet
}

fn truncate_chars(content: &str, char_limit: usize) -> String {
    let total_chars = content.chars().count();
    if total_chars <= char_limit {
        return content.to_string();
    }

    let end_byte = byte_index_for_char(content, char_limit);
    format!("{}...", &content[..end_byte])
}

fn byte_index_for_char(content: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }

    content
        .char_indices()
        .nth(char_index)
        .map(|(byte_index, _)| byte_index)
        .unwrap_or(content.len())
}

fn count_case_insensitive_matches(content: &str, query: &str) -> usize {
    let query_lower = query.to_lowercase();
    if query_lower.is_empty() {
        return 0;
    }

    let (content_lower, _, _) = lowercase_with_byte_map(content);
    content_lower.match_indices(&query_lower).count()
}

fn first_case_insensitive_match_range(content: &str, query: &str) -> Option<(usize, usize)> {
    let query_lower = query.to_lowercase();
    if query_lower.is_empty() {
        return None;
    }

    let (content_lower, lower_starts, lower_ends) = lowercase_with_byte_map(content);
    let lower_start = content_lower.find(&query_lower)?;
    let lower_end = lower_start + query_lower.len();
    let original_start = lower_starts
        .get(lower_start)
        .copied()
        .unwrap_or(content.len());
    let original_end = lower_end
        .checked_sub(1)
        .and_then(|idx| lower_ends.get(idx).copied())
        .unwrap_or(content.len());

    Some((original_start, original_end))
}

fn lowercase_with_byte_map(content: &str) -> (String, Vec<usize>, Vec<usize>) {
    let mut lowered = String::new();
    let mut lower_starts = Vec::new();
    let mut lower_ends = Vec::new();
    let mut chars = content.char_indices().peekable();

    while let Some((start, ch)) = chars.next() {
        let end = chars
            .peek()
            .map(|(next_start, _)| *next_start)
            .unwrap_or(content.len());
        let lower = ch.to_lowercase().to_string();

        for _ in 0..lower.len() {
            lower_starts.push(start);
            lower_ends.push(end);
        }

        lowered.push_str(&lower);
    }

    (lowered, lower_starts, lower_ends)
}

fn open_directory(path: &Path) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(path);
        command
    };

    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(path);
        command
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("explorer");
        command.arg(path);
        command
    };

    let status = command
        .status()
        .map_err(|error| format!("Failed to open directory {}: {error}", path.display()))?;

    if status.success() {
        Ok(true)
    } else {
        Err(format!(
            "Failed to open directory {}: opener exited with status {status}",
            path.display()
        ))
    }
}

fn is_symlink(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "Failed to inspect file metadata {}: {error}",
            path.display()
        )),
    }
}

fn ensure_workspace_root_safe() -> Result<(), String> {
    ensure_path_not_symlink(&workspace_dir(), "workspace directory")
}

fn ensure_daily_memory_root_safe() -> Result<(), String> {
    ensure_workspace_root_safe()?;
    ensure_path_not_symlink(&daily_memory_dir(), "daily memory directory")
}

fn ensure_path_not_symlink(path: &Path, label: &str) -> Result<(), String> {
    if is_symlink(path)? {
        Err(format!(
            "Refusing to use symlinked {label}: {}",
            path.display()
        ))
    } else {
        Ok(())
    }
}
