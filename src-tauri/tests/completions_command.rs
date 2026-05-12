use serial_test::serial;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn seed_future_schema_database(config_dir: &Path) {
    fs::create_dir_all(config_dir).expect("create config dir");
    let db_path = config_dir.join("cc-switch.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open sqlite db");
    conn.execute("PRAGMA user_version = 999;", [])
        .expect("set future schema version");
}

fn run_cc_switch(
    home: &Path,
    config_dir: &Path,
    shell: &str,
    args: &[&str],
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cc-switch-tui"))
        .args(args)
        .env("HOME", home)
        .env("SHELL", shell)
        .env("CC_SWITCH_CONFIG_DIR", config_dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("run cc-switch")
}

fn marker_count(content: &str) -> usize {
    content.matches("# >>> cc-switch completions >>>").count()
}

fn bash_completion_file(home: &Path) -> PathBuf {
    home.join(".local/share/bash-completion/completions/cc-switch-tui")
}

fn bash_rc_file(home: &Path) -> PathBuf {
    home.join(".bashrc")
}

#[test]
#[serial]
fn raw_generator_path_still_supports_non_automated_shells_without_startup_state() {
    let home = TempDir::new().expect("create temp home");
    let config_dir = TempDir::new().expect("create temp config");
    seed_future_schema_database(config_dir.path());

    let output = run_cc_switch(
        home.path(),
        config_dir.path(),
        "/usr/bin/fish",
        &["completions", "fish"],
    );

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("complete -c cc-switch"),
        "unexpected stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
#[serial]
fn bash_lifecycle_commands_work_end_to_end_without_startup_state() {
    let home = TempDir::new().expect("create temp home");
    let config_dir = TempDir::new().expect("create temp config");
    seed_future_schema_database(config_dir.path());
    fs::write(bash_rc_file(home.path()), "alias ll='ls -al'\n").expect("seed bashrc");

    let install = run_cc_switch(
        home.path(),
        config_dir.path(),
        "/bin/bash",
        &["completions", "install", "--activate"],
    );
    assert!(
        install.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr)
    );
    let install_stdout = String::from_utf8_lossy(&install.stdout);
    assert!(install_stdout.contains("Detected shell: bash"));
    assert!(install_stdout.contains("Installed completion file:"));
    assert!(install_stdout.contains("Managed activation block:"));
    assert!(bash_completion_file(home.path()).exists());

    let rc_after_install = fs::read_to_string(bash_rc_file(home.path())).expect("read bashrc");
    assert_eq!(marker_count(&rc_after_install), 1);
    assert!(rc_after_install.contains("alias ll='ls -al'"));

    let reinstall = run_cc_switch(
        home.path(),
        config_dir.path(),
        "/bin/bash",
        &["completions", "install", "--activate"],
    );
    assert!(
        reinstall.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&reinstall.stdout),
        String::from_utf8_lossy(&reinstall.stderr)
    );
    let rc_after_reinstall = fs::read_to_string(bash_rc_file(home.path())).expect("read bashrc");
    assert_eq!(marker_count(&rc_after_reinstall), 1);

    let status = run_cc_switch(
        home.path(),
        config_dir.path(),
        "/bin/bash",
        &["completions", "status"],
    );
    assert!(
        status.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(status_stdout.contains("Managed activation block: yes"));
    assert!(status_stdout.contains("Current shell likely needs reload: yes"));

    let uninstall = run_cc_switch(
        home.path(),
        config_dir.path(),
        "/bin/bash",
        &["completions", "uninstall"],
    );
    assert!(
        uninstall.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&uninstall.stdout),
        String::from_utf8_lossy(&uninstall.stderr)
    );
    assert!(!bash_completion_file(home.path()).exists());
    let rc_after_uninstall =
        fs::read_to_string(bash_rc_file(home.path())).expect("read bashrc after uninstall");
    assert!(!rc_after_uninstall.contains("# >>> cc-switch completions >>>"));
    assert!(rc_after_uninstall.contains("alias ll='ls -al'"));
}

#[test]
#[serial]
fn generator_path_rejects_lifecycle_flags_cleanly() {
    let home = TempDir::new().expect("create temp home");
    let config_dir = TempDir::new().expect("create temp config");

    let output = run_cc_switch(
        home.path(),
        config_dir.path(),
        "/bin/bash",
        &["completions", "bash", "--activate"],
    );

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument '--activate'"));
}
