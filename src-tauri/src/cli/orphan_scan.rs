use std::fs;
use std::path::{Path, PathBuf};

/// Information extracted from a temp file/directory name.
struct TempEntryInfo {
    path: PathBuf,
    pid: u32,
    nanos: u128,
}

/// Scan the temp directory for orphaned cc-switch temp files/directories
/// and clean them up. Returns the number of entries removed.
///
/// This is a best-effort operation: errors are logged but never propagated.
pub fn scan_and_clean(temp_dir: &Path) -> usize {
    let entries = match collect_cc_switch_entries(temp_dir) {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!(target: "orphan_scan", "Failed to read temp dir {}: {}", temp_dir.display(), e);
            return 0;
        }
    };

    let mut cleaned = 0;
    for entry in entries {
        if should_clean(&entry) {
            if let Err(e) = remove_entry(&entry.path) {
                log::warn!(target: "orphan_scan", "Failed to remove {}: {}", entry.path.display(), e);
            } else {
                log::debug!(target: "orphan_scan", "Cleaned orphaned temp entry: {}", entry.path.display());
                cleaned += 1;
            }
        }
    }

    cleaned
}

fn collect_cc_switch_entries(temp_dir: &Path) -> Result<Vec<TempEntryInfo>, std::io::Error> {
    let mut entries = Vec::new();
    let dir = fs::read_dir(temp_dir)?;

    for entry in dir {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => continue,
        };

        if let Some(info) = parse_cc_switch_name(name_str, entry.path()) {
            entries.push(info);
        }
    }

    Ok(entries)
}

fn parse_cc_switch_name(name: &str, path: PathBuf) -> Option<TempEntryInfo> {
    let rest = name
        .strip_prefix("cc-switch-claude-")
        .or_else(|| name.strip_prefix("cc-switch-codex-"))?;
    let rest = rest.strip_suffix(".json").unwrap_or(rest);
    let parts: Vec<&str> = rest.split('-').collect();
    if parts.len() < 2 {
        return None;
    }

    let nanos = parts.last()?.parse::<u128>().ok()?;
    let pid = parts.get(parts.len() - 2)?.parse::<u32>().ok()?;

    Some(TempEntryInfo { path, pid, nanos })
}

fn should_clean(entry: &TempEntryInfo) -> bool {
    // Living PID = leave alone. Dead PID = clean immediately: the file's
    // owner process is gone, so the file is a true orphan regardless of age.
    !is_pid_alive(entry.pid, entry.nanos)
}

#[cfg(windows)]
pub(crate) fn current_process_creation_time_nanos() -> u128 {
    use windows_sys::Win32::Foundation::{FILETIME, GetLastError};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    unsafe {
        let mut creation_time = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit_time = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut kernel_time = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut user_time = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };

        let result = GetProcessTimes(
            GetCurrentProcess(),
            &mut creation_time,
            &mut exit_time,
            &mut kernel_time,
            &mut user_time,
        );

        if result == 0 {
            // Fall back to current time if we can't read process creation time.
            // This is extremely unlikely but keeps the function infallible.
            log::warn!(
                target: "orphan_scan",
                "GetProcessTimes failed ({}); falling back to SystemTime::now()",
                GetLastError()
            );
            return std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
        }

        let filetime_to_nanos = |ft: &FILETIME| -> u128 {
            let low = ft.dwLowDateTime as u64;
            let high = ft.dwHighDateTime as u64;
            let intervals = (high << 32) | low;
            let nanos_since_1601 = intervals as u128 * 100;
            nanos_since_1601.saturating_sub(11644473600_000_000_000u128)
        };

        filetime_to_nanos(&creation_time)
    }
}

#[cfg(windows)]
fn is_pid_alive(pid: u32, file_nanos: u128) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, FILETIME,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            let err = GetLastError();
            if err == ERROR_INVALID_PARAMETER {
                return false;
            }
            return true;
        }

        let mut creation_time = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit_time = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut kernel_time = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut user_time = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };

        let result = GetProcessTimes(
            handle,
            &mut creation_time,
            &mut exit_time,
            &mut kernel_time,
            &mut user_time,
        );

        CloseHandle(handle);

        if result == 0 {
            return true;
        }

        let filetime_to_nanos = |ft: &FILETIME| -> u128 {
            let low = ft.dwLowDateTime as u64;
            let high = ft.dwHighDateTime as u64;
            let intervals = (high << 32) | low;
            let nanos_since_1601 = intervals as u128 * 100;
            nanos_since_1601.saturating_sub(11644473600_000_000_000u128)
        };

        let creation_nanos = filetime_to_nanos(&creation_time);

        // Exact match: the filename stores the process creation time, so a
        // living process with the same PID must have the exact same creation
        // time. Any mismatch means the PID has been reused by a different
        // process.
        creation_nanos == file_nanos
    }
}

#[cfg(unix)]
fn is_pid_alive(pid: u32, _file_nanos: u128) -> bool {
    unsafe {
        let result = libc::kill(pid as i32, 0);
        if result == 0 {
            return true;
        }
        let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        // EPERM means the process exists but we lack permission to signal it.
        // This branch is confirmed by code review; direct unit-testing would
        // require a process owned by another user, which is not feasible in
        // a standard test environment.
        err == libc::EPERM as i32
    }
}

#[cfg(not(any(windows, unix)))]
fn is_pid_alive(_pid: u32, _file_nanos: u128) -> bool {
    true
}

fn remove_entry(path: &Path) -> Result<(), std::io::Error> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

    #[test]
    fn parse_claude_filename() {
        let info = parse_cc_switch_name(
            "cc-switch-claude-demo-12345-1714137600000000000.json",
            PathBuf::from("/tmp/test"),
        )
        .unwrap();
        assert_eq!(info.pid, 12345);
        assert_eq!(info.nanos, 1714137600000000000);
    }

    #[test]
    fn parse_codex_dirname() {
        let info = parse_cc_switch_name(
            "cc-switch-codex-demo-12345-1714137600000000000",
            PathBuf::from("/tmp/test"),
        )
        .unwrap();
        assert_eq!(info.pid, 12345);
        assert_eq!(info.nanos, 1714137600000000000);
    }

    #[test]
    fn parse_rejects_non_cc_switch() {
        assert!(parse_cc_switch_name("some-other-file.json", PathBuf::from("/tmp/test")).is_none());
    }

    #[test]
    fn parse_rejects_other_cc_switch_prefix() {
        assert!(
            parse_cc_switch_name(
                "cc-switch-other-app-12345-1714137600000000000.json",
                PathBuf::from("/tmp/test")
            )
            .is_none()
        );
    }

    #[test]
    fn parse_rejects_invalid_pid() {
        assert!(parse_cc_switch_name(
            "cc-switch-claude-demo-notapid-1714137600000000000.json",
            PathBuf::from("/tmp/test")
        )
        .is_none());
    }

    #[test]
    fn dead_pid_triggers_clean() {
        let old_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .saturating_sub(25 * 60 * 60 * 1_000_000_000u128);
        let entry = TempEntryInfo {
            path: PathBuf::from("/tmp/cc-switch-claude-demo-99999-0.json"),
            pid: 99999,
            nanos: old_nanos,
        };
        assert!(should_clean(&entry));
    }

    #[test]
    fn dead_pid_with_fresh_file_triggers_clean() {
        // No TTL: a dead PID's file is cleaned immediately, even if just written.
        let recent_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let entry = TempEntryInfo {
            path: PathBuf::from("/tmp/cc-switch-claude-demo-99999-0.json"),
            pid: 99999,
            nanos: recent_nanos,
        };
        assert!(should_clean(&entry));
    }

    #[test]
    fn fresh_file_no_clean() {
        #[cfg(windows)]
        let recent_nanos = current_process_creation_time_nanos();
        #[cfg(not(windows))]
        let recent_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let entry = TempEntryInfo {
            path: PathBuf::from("/tmp/cc-switch-claude-demo-1-0.json"),
            pid: std::process::id(),
            nanos: recent_nanos,
        };
        assert!(!should_clean(&entry));
    }

    #[cfg(unix)]
    #[test]
    fn alive_pid_old_file_no_clean() {
        // On Windows, is_pid_alive also validates creation time, so an old
        // nanos with the current PID would be treated as PID reuse and
        // correctly considered dead. This test is Unix-only because kill(0)
        // does not check creation time.
        let old_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .saturating_sub(25 * 60 * 60 * 1_000_000_000u128);
        let entry = TempEntryInfo {
            path: PathBuf::from("/tmp/cc-switch-claude-demo-1-0.json"),
            pid: std::process::id(),
            nanos: old_nanos,
        };
        assert!(!should_clean(&entry));
    }

    #[cfg(windows)]
    #[test]
    fn windows_pid_reuse_detected_by_creation_time() {
        // Windows is_pid_alive validates creation time, so a living PID
        // with a mismatched (old) nanos is treated as PID reuse and
        // considered dead. This is the Windows counterpart to the
        // Unix-only alive_pid_old_file_no_clean test.
        let old_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .saturating_sub(25 * 60 * 60 * 1_000_000_000u128);
        let entry = TempEntryInfo {
            path: PathBuf::from("/tmp/cc-switch-claude-demo-1-0.json"),
            pid: std::process::id(),
            nanos: old_nanos,
        };
        // Current process creation time != old_nanos, so PID reuse
        assert!(should_clean(&entry));
    }

    #[test]
    fn scan_and_clean_removes_orphaned_files() {
        let temp = TempDir::new().expect("create temp dir");

        let old_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .saturating_sub(25 * 60 * 60 * 1_000_000_000u128);
        let orphan = temp
            .path()
            .join(format!("cc-switch-claude-demo-99999-{old_nanos}.json"));
        std::fs::write(&orphan, "{}").expect("write orphan file");

        let cleaned = scan_and_clean(temp.path());
        assert_eq!(cleaned, 1);
        assert!(!orphan.exists());
    }

    #[test]
    fn scan_and_clean_keeps_non_cc_switch_files() {
        let temp = TempDir::new().expect("create temp dir");
        let other = temp.path().join("some-other-file.txt");
        std::fs::write(&other, "hello").expect("write other file");

        let cleaned = scan_and_clean(temp.path());
        assert_eq!(cleaned, 0);
        assert!(other.exists());
    }

    #[test]
    fn scan_and_clean_removes_orphaned_dirs() {
        let temp = TempDir::new().expect("create temp dir");

        let old_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .saturating_sub(25 * 60 * 60 * 1_000_000_000u128);
        let orphan = temp
            .path()
            .join(format!("cc-switch-codex-demo-99999-{old_nanos}"));
        std::fs::create_dir(&orphan).expect("create orphan dir");
        std::fs::write(orphan.join("config.toml"), "model = \"test\"\n")
            .expect("write inside orphan dir");

        let cleaned = scan_and_clean(temp.path());
        assert_eq!(cleaned, 1);
        assert!(!orphan.exists());
    }
}
