mod support;

use support::ensure_test_home;

#[test]
fn ensure_test_home_scopes_path_to_current_process() {
    let home = ensure_test_home();
    let file_name = home
        .file_name()
        .and_then(|name| name.to_str())
        .expect("test home should have a terminal directory name");
    let expected = format!("ccs-t-{}", std::process::id());

    assert!(
        file_name == expected,
        "test home directory should use a process-scoped name for cross-binary isolation: expected {expected}, got {file_name}"
    );
}
