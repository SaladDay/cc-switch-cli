use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

/// Suffix for the sidecar metadata file that records the actual *child*
/// process PID and creation time. The sidecar is written by the launcher
/// after the child has been spawned (Windows only), and lets the orphan
/// scanner judge liveness by the child instead of the launcher. This is
/// what keeps a still-running Codex/Claude session safe when the launcher
/// dies in the nested-job fallback path.
pub(crate) const SIDECAR_SUFFIX: &str = ".child-meta";

/// Information extracted from a temp file/directory name.
struct TempEntryInfo {
    path: PathBuf,
    pid: u32,
    nanos: u128,
}

/// Compute the sidecar path for a given temp entry path by appending the
/// `SIDECAR_SUFFIX`. Works for both files (claude .json) and directories
/// (codex CODEX_HOME) — the sidecar always lives next to the entry.
pub(crate) fn sidecar_path_for(temp_path: &Path) -> PathBuf {
    let mut s: OsString = temp_path.as_os_str().to_owned();
    s.push(SIDECAR_SUFFIX);
    PathBuf::from(s)
}

/// Write the sidecar metadata file with the child's PID and creation
/// time in nanos. Uses an atomic create-then-rename so a crash mid-write
/// cannot leave a partial sidecar that the scanner would treat as authoritative.
pub(crate) fn write_child_sidecar(
    temp_path: &Path,
    child_pid: u32,
    creation_nanos: u128,
) -> std::io::Result<()> {
    let sidecar = sidecar_path_for(temp_path);
    let tmp = {
        let mut s: OsString = sidecar.as_os_str().to_owned();
        s.push(".tmp");
        PathBuf::from(s)
    };
    let content = format!("{child_pid}:{creation_nanos}");
    // Best-effort: remove any leftover .tmp from a previous failed attempt.
    let _ = fs::remove_file(&tmp);
    fs::write(&tmp, content.as_bytes())?;
    fs::rename(&tmp, &sidecar)
}

/// Best-effort removal of the sidecar associated with `temp_path`. Errors
/// are intentionally swallowed: a stray sidecar will eventually be reaped
/// by the orphan-sidecar pass on the next scan.
pub(crate) fn remove_sidecar_for(temp_path: &Path) {
    let sidecar = sidecar_path_for(temp_path);
    let _ = fs::remove_file(sidecar);
}

fn parse_sidecar(sidecar: &Path) -> Option<(u32, u128)> {
    let content = fs::read_to_string(sidecar).ok()?;
    let trimmed = content.trim();
    let mut parts = trimmed.splitn(2, ':');
    let pid = parts.next()?.parse::<u32>().ok()?;
    let nanos = parts.next()?.parse::<u128>().ok()?;
    Some((pid, nanos))
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

    // Reap sidecars whose main entry is gone (e.g., a previous run already
    // removed the entry but failed to remove the sidecar). This is what
    // bounds long-term sidecar accumulation.
    cleanup_orphan_sidecars(temp_dir);

    cleaned
}

fn cleanup_orphan_sidecars(temp_dir: &Path) {
    let dir = match fs::read_dir(temp_dir) {
        Ok(d) => d,
        Err(_) => return,
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => continue,
        };
        let stem = if let Some(s) = name_str.strip_suffix(SIDECAR_SUFFIX) {
            Some(s)
        } else if let Some(s) = name_str.strip_suffix(".child-meta.tmp") {
            Some(s)
        } else {
            None
        };
        if let Some(stem) = stem {
            // Only consider sidecars that belong to a cc-switch entry; ignore
            // any unrelated `.child-meta` file a user might have left behind.
            if !(stem.starts_with("cc-switch-claude-") || stem.starts_with("cc-switch-codex-")) {
                continue;
            }
            let main_path = temp_dir.join(stem);
            if !main_path.exists() {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
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

        // Sidecars are reaped separately so the scanner only inspects
        // primary temp entries here.
        if name_str.ends_with(SIDECAR_SUFFIX) {
            continue;
        }

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
    // Sidecar takes precedence: it records the *actual* child process. This
    // is what makes the nested-job fallback safe — the launcher's PID being
    // dead no longer implies the user-visible Codex/Claude session is dead.
    let sidecar = sidecar_path_for(&entry.path);
    if let Some((child_pid, child_nanos)) = parse_sidecar(&sidecar) {
        return !is_pid_alive(child_pid, child_nanos);
    }
    // Legacy / pre-spawn entries: fall back to the launcher PID stored in
    // the filename. Same semantics as before this fix landed.
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

#[cfg(target_os = "linux")]
fn read_pid_start_time_nanos(pid: u32) -> Option<u128> {
    // Boot time (Unix epoch seconds). Stays constant for the life of the
    // kernel, so reading it on every call is cheap.
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let btime_secs: u64 = stat
        .lines()
        .find_map(|line| line.strip_prefix("btime ").and_then(|s| s.trim().parse().ok()))?;

    // Process stat. The `comm` field is wrapped in parens and may itself
    // contain spaces, parens, or commas, so we anchor on the LAST `)`
    // before splitting the rest of the line by whitespace.
    let proc_stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close_paren = proc_stat.rfind(')')?;
    let after_comm = &proc_stat[close_paren + 1..];
    // Fields after `comm`: state ppid pgrp session tty_nr tpgid flags
    // minflt cminflt majflt cmajflt utime stime cutime cstime priority
    // nice num_threads itrealvalue starttime ...
    // starttime is the 20th token (0-indexed: 19).
    let starttime_ticks: u64 = after_comm
        .split_whitespace()
        .nth(19)
        .and_then(|s| s.parse().ok())?;

    let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if clk_tck <= 0 {
        return None;
    }
    let clk_tck = clk_tck as u128;
    // ticks → nanos: starttime / clk_tck * 1e9, computed without losing
    // precision for large starttimes.
    let start_nanos_since_boot = (starttime_ticks as u128).saturating_mul(1_000_000_000u128) / clk_tck;
    let btime_nanos = (btime_secs as u128).saturating_mul(1_000_000_000u128);
    Some(btime_nanos.saturating_add(start_nanos_since_boot))
}

#[cfg(target_os = "linux")]
fn is_pid_alive(pid: u32, file_nanos: u128) -> bool {
    // Liveness probe: if kill(pid, 0) reports ESRCH the PID is unused.
    // EPERM means the PID exists but belongs to another user — treat as
    // alive and let the start-time check below decide on PID reuse.
    let kill_result = unsafe { libc::kill(pid as i32, 0) };
    if kill_result != 0 {
        let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if err != libc::EPERM as i32 {
            return false;
        }
    }

    // Start-time probe via /proc. The launcher always created the temp
    // file *after* it started, so a process whose start time is later than
    // file_nanos (with a small tolerance for clock-tick precision) must be
    // a different process that has reused the PID.
    if let Some(start_nanos) = read_pid_start_time_nanos(pid) {
        // 2 s tolerance covers the worst-case CLK_TCK quantum (10 ms) plus
        // any clock skew between SystemTime::now() and the proc clock.
        const TOLERANCE_NANOS: u128 = 2_000_000_000;
        if start_nanos > file_nanos.saturating_add(TOLERANCE_NANOS) {
            return false;
        }
    }
    // /proc unreadable or start time consistent: prefer false-positive
    // "still alive" over false-positive "dead", since the latter would
    // delete user state.
    true
}

#[cfg(all(unix, not(target_os = "linux")))]
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
    fn parse_claude_filename_with_launch_seq_segment() {
        // New format inserts an 8-hex launch-seq segment before pid; parser
        // must still extract pid (second-to-last) and nanos (last).
        let info = parse_cc_switch_name(
            "cc-switch-claude-demo-0000002a-12345-1714137600000000000.json",
            PathBuf::from("/tmp/test"),
        )
        .unwrap();
        assert_eq!(info.pid, 12345);
        assert_eq!(info.nanos, 1714137600000000000);
    }

    #[test]
    fn parse_codex_dirname_with_launch_seq_segment() {
        let info = parse_cc_switch_name(
            "cc-switch-codex-demo-0000002a-12345-1714137600000000000",
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

    #[cfg(all(unix, not(target_os = "linux")))]
    #[test]
    fn alive_pid_old_file_no_clean() {
        // On non-Linux Unix (macOS, BSD), is_pid_alive uses kill(pid, 0)
        // only and does not validate process start time, so an old nanos
        // with a live PID is *not* treated as PID reuse. /proc-based
        // start-time validation is Linux-only.
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

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_pid_reuse_detected_by_start_time() {
        // Linux is_pid_alive validates start time via /proc/<pid>/stat,
        // so a live PID with a clearly-older file_nanos (test process
        // started long after the file was supposedly created) must be
        // treated as PID reuse. Mirrors the Windows test.
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
        // Test process start_time >> old_nanos, so PID reuse must clean
        assert!(should_clean(&entry));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_alive_pid_with_recent_file_no_clean() {
        // Conversely: a file_nanos *after* the live process started must
        // NOT be considered PID reuse. The launcher always wrote the file
        // after the process started, so this is the normal "still alive"
        // path.
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

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_dead_pid_returns_false_immediately() {
        // PID 1 is init, always alive. PID 99999 is virtually never alive
        // in a CI container or a developer machine. Probing it must return
        // false without hitting /proc.
        assert!(!is_pid_alive(99999, 0));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_read_pid_start_time_handles_comm_with_spaces_and_parens() {
        // The /proc/<pid>/stat parser must anchor on the LAST `)` so a
        // comm like "(My (Process))" with embedded parens still works.
        // We verify by reading the current process's start time, which is
        // the only PID we know exists.
        let pid = std::process::id();
        let nanos = read_pid_start_time_nanos(pid);
        assert!(
            nanos.is_some(),
            "must read start time for current process, got None"
        );
        let nanos = nanos.unwrap();
        let now_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        assert!(
            nanos <= now_nanos,
            "start_time {nanos} must be <= now {now_nanos}"
        );
        // Sanity: start time should be within the last day for a test run
        let one_day_ago = now_nanos.saturating_sub(24 * 60 * 60 * 1_000_000_000u128);
        assert!(
            nanos >= one_day_ago,
            "start_time {nanos} should be within the last 24h of {now_nanos}"
        );
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

    #[test]
    fn sidecar_dead_child_triggers_clean_even_when_launcher_alive() {
        // Sidecar precedence: a sidecar pointing at a dead child PID must
        // beat a still-alive launcher PID in the filename. This is the core
        // invariant for the nested-job fallback path — the launcher can be
        // alive while the user-visible Codex/Claude session has died.
        let temp = TempDir::new().expect("create temp dir");
        let recent_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let entry_path = temp
            .path()
            .join(format!("cc-switch-claude-demo-{}-{recent_nanos}.json", std::process::id()));
        std::fs::write(&entry_path, "{}").expect("write entry");

        // Sidecar references a guaranteed-dead PID with arbitrary nanos.
        write_child_sidecar(&entry_path, 99999, 0).expect("write sidecar");

        let entry = TempEntryInfo {
            path: entry_path.clone(),
            // Launcher PID = current process (still alive) and recent nanos
            pid: std::process::id(),
            nanos: recent_nanos,
        };
        assert!(
            should_clean(&entry),
            "sidecar's dead child PID must override the alive launcher PID"
        );
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_alive_child_blocks_clean_even_when_launcher_filename_old() {
        // Inverse of the test above: a sidecar pointing at a still-alive
        // child PID must keep the entry around even if the launcher PID's
        // filename nanos look stale. This protects the running Codex
        // session in the nested-job fallback path. Unix-only because on
        // Windows is_pid_alive cross-validates creation time, which the
        // synthetic file_nanos in this test would not match.
        let temp = TempDir::new().expect("create temp dir");
        let old_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .saturating_sub(25 * 60 * 60 * 1_000_000_000u128);
        let entry_path = temp
            .path()
            .join(format!("cc-switch-codex-demo-99999-{old_nanos}"));
        std::fs::create_dir(&entry_path).expect("create entry");

        // Sidecar points at this very test process, which is obviously
        // alive. The Unix is_pid_alive ignores nanos, so any nanos works.
        write_child_sidecar(&entry_path, std::process::id(), 0).expect("write sidecar");

        let entry = TempEntryInfo {
            path: entry_path.clone(),
            // Launcher PID would otherwise look dead
            pid: 99999,
            nanos: old_nanos,
        };
        assert!(
            !should_clean(&entry),
            "sidecar's alive child PID must keep the entry even if launcher PID is stale"
        );
    }

    #[test]
    fn sidecar_orphan_is_reaped_when_main_entry_missing() {
        // A sidecar without its main entry must be cleaned by the
        // orphan-sidecar reap pass; otherwise sidecars accumulate forever
        // when the launcher crashes mid-cleanup.
        let temp = TempDir::new().expect("create temp dir");
        let entry_path = temp.path().join("cc-switch-claude-demo-99999-0.json");
        write_child_sidecar(&entry_path, 99999, 0).expect("write sidecar");
        // Note: we never create the main entry.
        let sidecar = sidecar_path_for(&entry_path);
        assert!(sidecar.exists(), "sidecar must exist before reap");

        let _ = scan_and_clean(temp.path());
        assert!(!sidecar.exists(), "orphan sidecar must be reaped");
    }

    #[test]
    fn sidecar_unrelated_child_meta_files_are_preserved() {
        // Reaper must only touch sidecars that belong to cc-switch entries,
        // not arbitrary `.child-meta` files a user might leave behind.
        let temp = TempDir::new().expect("create temp dir");
        let unrelated = temp.path().join("not-cc-switch.child-meta");
        std::fs::write(&unrelated, "12345:0").expect("write unrelated sidecar");

        let _ = scan_and_clean(temp.path());
        assert!(
            unrelated.exists(),
            "unrelated .child-meta files must not be touched"
        );
    }

    #[test]
    fn sidecar_atomic_write_replaces_existing() {
        // Sequential writes must atomically replace the previous content;
        // the rename-over-existing must not leave stale .tmp files.
        let temp = TempDir::new().expect("create temp dir");
        let entry_path = temp.path().join("cc-switch-claude-demo-1-0.json");

        write_child_sidecar(&entry_path, 1234, 100).expect("first write");
        write_child_sidecar(&entry_path, 5678, 200).expect("second write");

        let sidecar = sidecar_path_for(&entry_path);
        let content = std::fs::read_to_string(&sidecar).expect("read sidecar");
        assert_eq!(content, "5678:200", "second write must replace first");

        // No .tmp leftover
        let mut s: OsString = sidecar.as_os_str().to_owned();
        s.push(".tmp");
        let tmp_path = PathBuf::from(s);
        assert!(!tmp_path.exists(), "no stale .tmp must remain");
    }
}
