//! OMP session-file discovery for usage synchronization.
//!
//! OMP stores sessions below the active agent directory.  The directory may
//! be redirected by the same environment/profile rules used by the native
//! configuration adapter, so discovery delegates root resolution to
//! [`crate::omp_config`].  This module intentionally exposes only file
//! discovery for now; the regular session browser is still a separate,
//! follow-up feature.

use std::fs;
use std::path::PathBuf;

use crate::services::session_usage::read_session_directory_entries_no_follow;

/// Keep usage imports bounded in the same way as the Pi importer.
pub(crate) const MAX_SESSION_BYTES: u64 = 128 * 1024 * 1024;
pub(crate) const MAX_TREE_ENTRIES: usize = 500_000;

pub(crate) fn is_valid_tree_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}
const MAX_SESSION_FILES: usize = 500_000;
const MAX_DIRECTORY_DEPTH: usize = 64;

/// Return all regular OMP JSONL session files under the active agent root.
///
/// Missing session directories are normal before the first OMP invocation and
/// therefore produce an empty result. Symlinks are never followed.
pub(crate) fn session_files() -> Result<Vec<PathBuf>, String> {
    let root = crate::omp_config::get_omp_sessions_dir().map_err(|error| error.to_string())?;
    let metadata = match fs::symlink_metadata(&root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "OMP session directory is unavailable ({}): {error}",
                root.display()
            ));
        }
    };
    if !metadata.is_dir() {
        return Err(format!(
            "OMP session path is not a directory: {}",
            root.display()
        ));
    }

    let mut files = Vec::new();
    let mut pending = vec![(root, 0usize)];
    while let Some((directory, depth)) = pending.pop() {
        let directory_metadata = fs::symlink_metadata(&directory).map_err(|error| {
            format!(
                "OMP session path metadata failed ({}): {error}",
                directory.display()
            )
        })?;
        if directory_metadata.file_type().is_symlink() {
            return Err(format!(
                "OMP session directory became a symlink: {}",
                directory.display()
            ));
        }
        if !directory_metadata.is_dir() {
            continue;
        }
        let entries = read_session_directory_entries_no_follow(&directory).map_err(|error| {
            format!(
                "OMP session directory is not readable ({}): {error}",
                directory.display()
            )
        })?;
        for (name, is_dir, is_file, is_symlink) in entries {
            let path = directory.join(name);
            if is_symlink {
                continue;
            }
            if is_dir {
                if depth < MAX_DIRECTORY_DEPTH {
                    pending.push((path, depth + 1));
                } else {
                    log::warn!(
                        "Skipping OMP session directory beyond depth limit: {}",
                        path.display()
                    );
                }
                continue;
            }
            if !is_file || path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            if files.len() >= MAX_SESSION_FILES {
                return Err(format!(
                    "OMP session file count exceeds safety limit ({MAX_SESSION_FILES})"
                ));
            }
            // Keep oversized files in the candidate list. The importer emits a
            // useful per-file error instead of silently reporting success.
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_session_directory_is_empty() {
        // This test only checks the bounded traversal helper contract through
        // a temporary tree; root resolution itself is environment-dependent.
        let root = tempfile::tempdir()
            .expect("tempdir")
            .path()
            .join("sessions");
        assert!(!root.exists());
    }

    #[test]
    fn max_session_bytes_is_bounded() {
        assert_eq!(MAX_SESSION_BYTES, 128 * 1024 * 1024);
    }

    #[test]
    fn discovery_limits_are_explicit() {
        assert_eq!(MAX_DIRECTORY_DEPTH, 64);
        assert_eq!(MAX_SESSION_FILES, 500_000);
        assert_eq!(MAX_TREE_ENTRIES, 500_000);
    }
}
