use cc_switch_lib::commands::workspace::{
    delete_daily_memory_file, list_daily_memory_files, open_workspace_directory,
    read_daily_memory_file, read_workspace_file, search_daily_memory_files,
    write_daily_memory_file, write_workspace_file,
};
use serde_json::Value;

#[path = "support.rs"]
mod support;

use support::{ensure_test_home, lock_test_mutex, reset_test_fs};

fn workspace_dir() -> std::path::PathBuf {
    ensure_test_home().join(".openclaw").join("workspace")
}

fn memory_dir() -> std::path::PathBuf {
    workspace_dir().join("memory")
}

#[cfg(unix)]
fn symlink_file(original: &std::path::Path, link: &std::path::Path) {
    std::os::unix::fs::symlink(original, link).expect("create symlink");
}

#[cfg(unix)]
fn symlink_dir(original: &std::path::Path, link: &std::path::Path) {
    std::os::unix::fs::symlink(original, link).expect("create dir symlink");
}

#[cfg(unix)]
fn set_unreadable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)
        .expect("read metadata")
        .permissions();
    permissions.set_mode(0o000);
    std::fs::set_permissions(path, permissions).expect("set unreadable permissions");
}

fn json_keys(value: &Value) -> Vec<&str> {
    let mut keys = value
        .as_object()
        .expect("metadata should serialize to an object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys
}

#[test]
fn workspace_file_missing_allowed_file_returns_none() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    let result = read_workspace_file("AGENTS.md".to_string()).expect("read should succeed");

    assert_eq!(result, None);
}

#[test]
fn workspace_file_write_creates_directory_and_round_trips_exact_content() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    let content = "alpha\n\nbeta\ngamma";

    write_workspace_file("MEMORY.md".to_string(), content.to_string())
        .expect("write should succeed");

    assert!(workspace_dir().is_dir(), "workspace dir should be created");
    assert_eq!(
        std::fs::read_to_string(workspace_dir().join("MEMORY.md")).expect("read raw file"),
        content
    );

    let read_back = read_workspace_file("MEMORY.md".to_string()).expect("read should succeed");
    assert_eq!(read_back.as_deref(), Some(content));
}

#[test]
fn workspace_file_disallowed_filename_is_rejected() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    let read_err = read_workspace_file("../../secret.txt".to_string())
        .expect_err("read should reject disallowed filename");
    assert!(
        read_err.contains("Invalid workspace filename"),
        "unexpected read error: {read_err}"
    );

    let write_err = write_workspace_file("NOT_ALLOWED.md".to_string(), "x".to_string())
        .expect_err("write should reject disallowed filename");
    assert!(
        write_err.contains("Invalid workspace filename"),
        "unexpected write error: {write_err}"
    );
}

#[cfg(unix)]
#[test]
fn workspace_file_read_rejects_symlink() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    std::fs::create_dir_all(workspace_dir()).expect("create workspace dir");
    let outside = ensure_test_home().join("outside-workspace.txt");
    std::fs::write(&outside, "outside workspace secret").expect("write outside file");
    symlink_file(&outside, &workspace_dir().join("AGENTS.md"));

    let err = read_workspace_file("AGENTS.md".to_string())
        .expect_err("workspace read should reject symlinked file");
    assert!(err.contains("symlink"), "unexpected error: {err}");
}

#[cfg(unix)]
#[test]
fn workspace_operations_reject_symlinked_workspace_dir() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    std::env::set_var("CC_SWITCH_TEST_DISABLE_OPEN", "1");

    let outside_dir = ensure_test_home().join("outside-workspace-dir");
    std::fs::create_dir_all(&outside_dir).expect("create outside workspace dir");
    std::fs::create_dir_all(ensure_test_home().join(".openclaw")).expect("create openclaw dir");
    symlink_dir(&outside_dir, &workspace_dir());

    let write_err = write_workspace_file("AGENTS.md".to_string(), "content".to_string())
        .expect_err("write should reject symlinked workspace dir");
    assert!(
        write_err.contains("symlink"),
        "unexpected write error: {write_err}"
    );

    let read_err = read_workspace_file("AGENTS.md".to_string())
        .expect_err("read should reject symlinked workspace dir");
    assert!(
        read_err.contains("symlink"),
        "unexpected read error: {read_err}"
    );

    let open_err = open_workspace_directory((), "anything-else".to_string())
        .expect_err("open should reject symlinked workspace dir");
    assert!(
        open_err.contains("symlink"),
        "unexpected open error: {open_err}"
    );
}

#[test]
fn workspace_directory_memory_subdir_creates_memory_dir_and_returns_true() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    std::env::set_var("CC_SWITCH_TEST_DISABLE_OPEN", "1");

    let opened = open_workspace_directory((), "memory".to_string())
        .expect("opening memory directory should succeed");

    assert!(opened, "open command should report success");
    assert!(memory_dir().is_dir(), "memory dir should be created");
}

#[test]
fn workspace_directory_non_memory_subdir_falls_back_to_workspace_dir() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    std::env::set_var("CC_SWITCH_TEST_DISABLE_OPEN", "1");

    let opened = open_workspace_directory((), "anything-else".to_string())
        .expect("opening workspace directory should succeed");

    assert!(opened, "open command should report success");
    assert!(workspace_dir().is_dir(), "workspace dir should be created");
    assert!(
        !memory_dir().exists(),
        "non-memory subdir should not redirect into memory dir"
    );
}

#[test]
fn daily_memory_invalid_filename_is_rejected() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    let err = write_daily_memory_file("today.md".to_string(), "content".to_string())
        .expect_err("invalid filename should be rejected");
    assert!(
        err.contains("Invalid daily memory filename"),
        "unexpected error: {err}"
    );
}

#[test]
fn daily_memory_missing_file_returns_none() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    let result =
        read_daily_memory_file("2026-03-20.md".to_string()).expect("missing read should succeed");

    assert_eq!(result, None);
}

#[cfg(unix)]
#[test]
fn daily_memory_read_rejects_symlink() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    std::fs::create_dir_all(memory_dir()).expect("create memory dir");
    let outside = ensure_test_home().join("outside-memory.txt");
    std::fs::write(&outside, "outside memory secret").expect("write outside file");
    symlink_file(&outside, &memory_dir().join("2026-03-20.md"));

    let err = read_daily_memory_file("2026-03-20.md".to_string())
        .expect_err("daily memory read should reject symlinked file");
    assert!(err.contains("symlink"), "unexpected error: {err}");
}

#[cfg(unix)]
#[test]
fn daily_memory_operations_reject_symlinked_memory_dir() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    std::env::set_var("CC_SWITCH_TEST_DISABLE_OPEN", "1");

    std::fs::create_dir_all(workspace_dir()).expect("create workspace dir");
    let outside_dir = ensure_test_home().join("outside-memory-dir");
    std::fs::create_dir_all(&outside_dir).expect("create outside memory dir");
    symlink_dir(&outside_dir, &memory_dir());

    let write_err = write_daily_memory_file("2026-03-20.md".to_string(), "content".to_string())
        .expect_err("write should reject symlinked memory dir");
    assert!(
        write_err.contains("symlink"),
        "unexpected write error: {write_err}"
    );

    let read_err = read_daily_memory_file("2026-03-20.md".to_string())
        .expect_err("read should reject symlinked memory dir");
    assert!(
        read_err.contains("symlink"),
        "unexpected read error: {read_err}"
    );

    let list_err = list_daily_memory_files().expect_err("list should reject symlinked memory dir");
    assert!(
        list_err.contains("symlink"),
        "unexpected list error: {list_err}"
    );

    let search_err = search_daily_memory_files("content".to_string())
        .expect_err("search should reject symlinked memory dir");
    assert!(
        search_err.contains("symlink"),
        "unexpected search error: {search_err}"
    );

    let open_err = open_workspace_directory((), "memory".to_string())
        .expect_err("open should reject symlinked memory dir");
    assert!(
        open_err.contains("symlink"),
        "unexpected open error: {open_err}"
    );
}

#[test]
fn daily_memory_write_creates_directory_and_delete_is_idempotent() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    write_daily_memory_file(
        "2026-03-20.md".to_string(),
        "line one\n\nline two".to_string(),
    )
    .expect("write should succeed");

    assert!(memory_dir().is_dir(), "memory dir should be created");

    delete_daily_memory_file("2026-03-20.md".to_string()).expect("first delete should succeed");
    delete_daily_memory_file("2026-03-20.md".to_string())
        .expect("second delete should also succeed");
    assert!(
        !memory_dir().join("2026-03-20.md").exists(),
        "file should be absent after delete"
    );
}

#[test]
fn daily_memory_list_sorts_desc_and_reports_metadata_shape() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    write_daily_memory_file("2026-03-18.md".to_string(), "older".to_string())
        .expect("write older file");
    write_daily_memory_file("2026-03-20.md".to_string(), "newer preview".to_string())
        .expect("write newer file");
    write_daily_memory_file("2026-03-19.md".to_string(), "middle".to_string())
        .expect("write middle file");

    let files = list_daily_memory_files().expect("list should succeed");
    let filenames = files
        .iter()
        .map(|file| file.filename.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        filenames,
        vec!["2026-03-20.md", "2026-03-19.md", "2026-03-18.md"]
    );

    let metadata = serde_json::to_value(&files[0]).expect("serialize metadata");
    assert_eq!(
        json_keys(&metadata),
        vec!["date", "filename", "modifiedAt", "preview", "sizeBytes"]
    );
    assert_eq!(
        metadata.get("date").and_then(Value::as_str),
        Some("2026-03-20")
    );
    assert_eq!(
        metadata.get("preview").and_then(Value::as_str),
        Some("newer preview")
    );
}

#[cfg(unix)]
#[test]
fn daily_memory_list_and_search_ignore_symlinks() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    write_daily_memory_file("2026-03-20.md".to_string(), "safe content".to_string())
        .expect("write regular file");

    let outside = ensure_test_home().join("outside-memory-search.txt");
    std::fs::write(&outside, "safe keyword but outside").expect("write outside file");
    symlink_file(&outside, &memory_dir().join("2026-03-21.md"));

    let files = list_daily_memory_files().expect("list should succeed");
    let filenames = files
        .iter()
        .map(|file| file.filename.as_str())
        .collect::<Vec<_>>();
    assert_eq!(filenames, vec!["2026-03-20.md"]);

    let results = search_daily_memory_files("keyword".to_string()).expect("search should succeed");
    assert!(
        results.is_empty(),
        "search should ignore symlinked files outside the memory root"
    );
}

#[cfg(unix)]
#[test]
fn daily_memory_list_keeps_bad_files_visible_with_empty_preview() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    write_daily_memory_file("2026-03-20.md".to_string(), "safe content".to_string())
        .expect("write regular file");

    std::fs::write(memory_dir().join("2026-03-19.md"), [0xff, 0xfe, 0xfd])
        .expect("write invalid utf8 file");
    std::fs::write(memory_dir().join("2026-03-18.md"), "hidden content")
        .expect("write unreadable file");
    set_unreadable(&memory_dir().join("2026-03-18.md"));

    let files = list_daily_memory_files().expect("list should tolerate bad files");
    let filenames = files
        .iter()
        .map(|file| file.filename.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        filenames,
        vec!["2026-03-20.md", "2026-03-19.md", "2026-03-18.md"]
    );
    assert_eq!(files[0].preview, "safe content");
    assert_eq!(files[1].preview, "");
    assert_eq!(files[2].preview, "");
}

#[cfg(unix)]
#[test]
fn daily_memory_search_keeps_bad_files_visible_on_date_matches() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    write_daily_memory_file("2026-03-20.md".to_string(), "keyword here".to_string())
        .expect("write regular file");

    std::fs::write(memory_dir().join("2026-03-19.md"), [0xff, 0xfe, 0xfd])
        .expect("write invalid utf8 file");
    std::fs::write(memory_dir().join("2026-03-18.md"), "keyword but unreadable")
        .expect("write unreadable file");
    set_unreadable(&memory_dir().join("2026-03-18.md"));

    let results = search_daily_memory_files("2026-03".to_string())
        .expect("search should tolerate bad files on date matches");
    let filenames = results
        .iter()
        .map(|file| file.filename.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        filenames,
        vec!["2026-03-20.md", "2026-03-19.md", "2026-03-18.md"]
    );
    assert_eq!(results[0].snippet, "keyword here");
    assert_eq!(results[0].match_count, 0);
    assert_eq!(results[1].snippet, "");
    assert_eq!(results[1].match_count, 0);
    assert_eq!(results[2].snippet, "");
    assert_eq!(results[2].match_count, 0);
}

#[test]
fn daily_memory_metadata_serializes_modified_at_in_unix_seconds() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    write_daily_memory_file(
        "2026-03-20.md".to_string(),
        "serialized metadata".to_string(),
    )
    .expect("write file");

    let expected_seconds = std::fs::metadata(memory_dir().join("2026-03-20.md"))
        .expect("read metadata")
        .modified()
        .expect("read modified time")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("duration since epoch")
        .as_secs();

    let files = list_daily_memory_files().expect("list should succeed");
    let list_json = serde_json::to_value(&files[0]).expect("serialize list entry");
    assert_eq!(
        list_json.get("modifiedAt").and_then(Value::as_u64),
        Some(expected_seconds)
    );

    let results =
        search_daily_memory_files("serialized".to_string()).expect("search should succeed");
    let search_json = serde_json::to_value(&results[0]).expect("serialize search entry");
    assert_eq!(
        search_json.get("modifiedAt").and_then(Value::as_u64),
        Some(expected_seconds)
    );
}

#[test]
fn daily_memory_read_round_trips_exact_content() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    let content = "first line\n\nsecond line without trailing newline";
    write_daily_memory_file("2026-03-20.md".to_string(), content.to_string())
        .expect("write should succeed");

    let read_back =
        read_daily_memory_file("2026-03-20.md".to_string()).expect("read should succeed");
    assert_eq!(read_back.as_deref(), Some(content));
    assert_eq!(
        std::fs::read_to_string(memory_dir().join("2026-03-20.md")).expect("read raw file"),
        content
    );
}

#[test]
fn daily_memory_search_empty_query_returns_empty_results() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    write_daily_memory_file("2026-03-20.md".to_string(), "anything".to_string())
        .expect("write should succeed");

    let results = search_daily_memory_files("   ".to_string()).expect("search should succeed");
    assert!(results.is_empty(), "empty query should not return matches");
}

#[test]
fn daily_memory_search_is_case_insensitive_and_reports_metadata_shape() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    write_daily_memory_file(
        "2026-03-20.md".to_string(),
        "Alpha beta GAMMA\nsecond line".to_string(),
    )
    .expect("write should succeed");

    let results = search_daily_memory_files("gamma".to_string()).expect("search should succeed");
    assert_eq!(
        results.len(),
        1,
        "search should find content case-insensitively"
    );
    assert!(
        results[0].snippet.to_lowercase().contains("gamma"),
        "snippet should include the content match"
    );
    assert!(
        results[0].match_count >= 1,
        "match count should be populated"
    );

    let metadata = serde_json::to_value(&results[0]).expect("serialize search result");
    assert_eq!(
        json_keys(&metadata),
        vec![
            "date",
            "filename",
            "matchCount",
            "modifiedAt",
            "sizeBytes",
            "snippet",
        ]
    );
}

#[test]
fn daily_memory_search_matches_date_and_sorts_descending() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    write_daily_memory_file("2026-03-20.md".to_string(), "first day".to_string())
        .expect("write first file");
    write_daily_memory_file("2026-03-18.md".to_string(), "older day".to_string())
        .expect("write second file");
    write_daily_memory_file("2025-03-20.md".to_string(), "last year".to_string())
        .expect("write third file");

    let results =
        search_daily_memory_files("03-20".to_string()).expect("date search should succeed");

    let filenames = results
        .iter()
        .map(|result| result.filename.as_str())
        .collect::<Vec<_>>();
    assert_eq!(filenames, vec!["2026-03-20.md", "2025-03-20.md"]);
    assert_eq!(
        results[0].snippet, "first day",
        "date-only search should fall back to the file preview"
    );
}

#[test]
fn daily_memory_search_handles_utf8_boundary_snippets() {
    let _guard = lock_test_mutex();
    reset_test_fs();

    write_daily_memory_file(
        "2026-03-20.md".to_string(),
        "前缀🙂🙂🙂中间关键字🙂🙂🙂后缀".to_string(),
    )
    .expect("write should succeed");

    let results = search_daily_memory_files("关键字".to_string()).expect("search should succeed");

    assert_eq!(results.len(), 1, "utf-8 content should be searchable");
    assert!(results[0].snippet.contains("关键字"));
    assert!(results[0].snippet.contains("🙂"));
}
