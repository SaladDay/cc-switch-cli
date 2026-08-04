use serde_json::Value;
use serial_test::serial;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs};

fn run_cc_switch(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cc-switch"))
        .args(args)
        .env("HOME", home)
        .env("CC_SWITCH_CONFIG_DIR", home.join(".cc-switch"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("CODEX_HOME", home.join(".codex"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_RUNTIME_DIR", home.join(".runtime"))
        .env("XDG_STATE_HOME", home.join(".state"))
        .env("NO_COLOR", "1")
        .output()
        .expect("run cc-switch")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[serial]
fn codex_auth_preservation_commands_are_scriptable_and_do_not_touch_live_config() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();
    let codex_dir = home.join(".codex");
    fs::create_dir_all(&codex_dir).expect("create Codex config dir");
    let auth_path = codex_dir.join("auth.json");
    let config_path = codex_dir.join("config.toml");
    let auth_before =
        b"{\n  \"auth_mode\": \"chatgpt\",\n  \"tokens\": {\"access_token\": \"secret\"}\n}\n";
    let config_before = b"model_provider = \"custom\"\n";
    fs::write(&auth_path, auth_before).expect("seed auth.json");
    fs::write(&config_path, config_before).expect("seed config.toml");

    let enable = run_cc_switch(home, &["settings", "codex-auth-preservation", "enable"]);
    assert_success(&enable);
    assert!(String::from_utf8_lossy(&enable.stdout)
        .contains("Codex official login preservation enabled"));

    let show = run_cc_switch(
        home,
        &["settings", "codex-auth-preservation", "show", "--json"],
    );
    assert_success(&show);
    let payload: Value = serde_json::from_slice(&show.stdout).expect("parse JSON output");
    assert_eq!(
        payload["preserveCodexOfficialAuthOnSwitch"],
        Value::Bool(true)
    );

    let disable = run_cc_switch(home, &["settings", "codex-auth-preservation", "disable"]);
    assert_success(&disable);
    assert!(String::from_utf8_lossy(&disable.stdout)
        .contains("Codex official login preservation disabled"));

    let show_disabled = run_cc_switch(
        home,
        &["settings", "codex-auth-preservation", "show", "--json"],
    );
    assert_success(&show_disabled);
    let payload: Value =
        serde_json::from_slice(&show_disabled.stdout).expect("parse disabled JSON output");
    assert_eq!(
        payload["preserveCodexOfficialAuthOnSwitch"],
        Value::Bool(false)
    );

    assert_eq!(fs::read(&auth_path).expect("read auth.json"), auth_before);
    assert_eq!(
        fs::read(&config_path).expect("read config.toml"),
        config_before
    );
    assert!(
        !home.join(".cc-switch/cc-switch.db").exists(),
        "settings-only commands should not initialize the database"
    );
}
