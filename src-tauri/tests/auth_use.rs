use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};
#[path = "support.rs"]
mod support;

fn run(home: &Path, args: &[&str]) -> std::process::Output {
    command(home, args).output().unwrap()
}

fn command(home: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cc-switch"));
    cmd.args(args)
        .env("HOME", home)
        .env("CC_SWITCH_CONFIG_DIR", home.join(".cc-switch"))
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_RUNTIME_DIR", home.join(".runtime"))
        .env("XDG_STATE_HOME", home.join(".state"));
    cmd
}

#[test]
fn auth_use_persists_across_cli_processes_and_status_reports_active_account() {
    let _lock = support::lock_test_mutex();
    support::reset_test_fs();
    let home = support::ensure_test_home();
    fs::create_dir_all(home.join(".cc-switch")).unwrap();
    // Intentionally leave CODEX_HOME absent: activation must create the selected home.
    let access = format!(
        "e30.{}.sig",
        URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({"exp":chrono::Utc::now().timestamp()+3600})).unwrap()
        )
    );
    let mut accounts = serde_json::Map::new();
    for id in ["a", "b"] {
        accounts.insert(id.into(), json!({"account_id":id,"refresh_token":format!("secret-refresh-{id}"),"authenticated_at":1,
            "codex_auth":{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{
                "account_id":id,"refresh_token":format!("secret-refresh-{id}"),"access_token":access,"id_token":"secret-id-token"},
                "last_refresh":"2026-01-01T00:00:00Z"}}));
    }
    fs::write(
        home.join(".cc-switch/codex_oauth_auth.json"),
        serde_json::to_vec(&json!({"version":1,"accounts":accounts,"default_account_id":"a"}))
            .unwrap(),
    )
    .unwrap();
    for id in ["b", "a", "b"] {
        let output = run(home, &["--app", "codex", "auth", "use", id]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-"));
        let output = run(home, &["auth", "status", "--json"]);
        assert!(output.status.success());
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(status["active_codex_account_id"], id);
        assert_eq!(status["default_account_id"], id);
        let auth: Value =
            serde_json::from_slice(&fs::read(home.join(".codex/auth.json")).unwrap()).unwrap();
        assert_eq!(auth["tokens"]["account_id"], id);
    }
    assert!(run(home, &["auth", "default", "a"]).status.success());
    let status: Value =
        serde_json::from_slice(&run(home, &["auth", "status", "--json"]).stdout).unwrap();
    assert_eq!(status["default_account_id"], "a");
    assert_eq!(status["active_codex_account_id"], "b");
    let before = fs::read(home.join(".codex/auth.json")).unwrap();
    assert!(!run(home, &["auth", "use", "missing"]).status.success());
    assert_eq!(fs::read(home.join(".codex/auth.json")).unwrap(), before);
    // Credentials refreshed by cc-switch start must survive a later global switch.
    let db = cc_switch_lib::Database::init().unwrap();
    let current: Value = serde_json::from_slice(&before).unwrap();
    let mut provider = cc_switch_lib::Provider::with_id(
        "official".into(),
        "Official".into(),
        json!({"auth":current,"config":""}),
        None,
    );
    provider.category = Some("official".into());
    db.save_provider("codex", &provider).unwrap();
    db.set_current_provider("codex", "official").unwrap();
    let launch = home.join("launch");
    fs::create_dir_all(&launch).unwrap();
    let mut refreshed = current;
    refreshed["tokens"]["refresh_token"] = json!("secret-from-temp-launch");
    refreshed["last_refresh"] = json!(chrono::Utc::now().to_rfc3339());
    fs::write(
        launch.join("auth.json"),
        serde_json::to_vec(&refreshed).unwrap(),
    )
    .unwrap();
    let output = run(
        home,
        &[
            "internal",
            "capture-codex-temp",
            "official",
            launch.to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let after_capture: Value =
        serde_json::from_slice(&fs::read(home.join(".codex/auth.json")).unwrap()).unwrap();
    assert_eq!(
        after_capture["tokens"]["refresh_token"],
        "secret-from-temp-launch"
    );
    for id in ["a", "b"] {
        let output = run(home, &["auth", "use", id]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let restored: Value =
        serde_json::from_slice(&fs::read(home.join(".codex/auth.json")).unwrap()).unwrap();
    assert_eq!(
        restored["tokens"]["refresh_token"],
        "secret-from-temp-launch"
    );
    // A publication failure must remain recoverable across CLI process restarts.
    let primary = db.get_all_providers("codex").unwrap()["official"].clone();
    let mut secondary = primary.clone();
    secondary.id = "secondary".into();
    secondary.name = "Secondary".into();
    db.save_provider("codex", &secondary).unwrap();
    let mut next = primary.settings_config["auth"].clone();
    next["tokens"]["refresh_token"] = json!("secret-after-partial-sync");
    next["last_refresh"] = json!(chrono::Utc::now().to_rfc3339());
    fs::write(launch.join("auth.json"), serde_json::to_vec(&next).unwrap()).unwrap();
    use sha2::{Digest, Sha256};
    fs::write(
        launch.join(".auth-source"),
        format!(
            "{:x}",
            Sha256::digest(primary.settings_config["auth"].to_string().as_bytes())
        ),
    )
    .unwrap();
    let connection = rusqlite::Connection::open(home.join(".cc-switch/cc-switch.db")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_secondary BEFORE UPDATE OF settings_config ON providers WHEN OLD.id = 'secondary' BEGIN SELECT RAISE(ABORT, 'synthetic temporary failure'); END;").unwrap();
    let capture_args = [
        "internal",
        "capture-codex-temp",
        "official",
        launch.to_str().unwrap(),
        "--auth-only",
    ];
    assert!(!run(home, &capture_args).status.success());
    let store: Value =
        serde_json::from_slice(&fs::read(home.join(".cc-switch/codex_oauth_auth.json")).unwrap())
            .unwrap();
    assert!(store["accounts"]["b"].get("pending_native_sync").is_some());
    connection
        .execute_batch("DROP TRIGGER fail_secondary;")
        .unwrap();
    let retry = run(home, &capture_args);
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert_eq!(
        db.get_all_providers("codex").unwrap()["secondary"].settings_config["auth"]["tokens"]
            ["refresh_token"],
        "secret-after-partial-sync"
    );
    let store: Value =
        serde_json::from_slice(&fs::read(home.join(".cc-switch/codex_oauth_auth.json")).unwrap())
            .unwrap();
    assert!(store["accounts"]["b"].get("pending_native_sync").is_none());
    // Observe the child waiting on the lock before checking for premature writes.
    #[cfg(target_os = "linux")]
    {
        let initial = db.get_all_providers("codex").unwrap()["official"]
            .settings_config
            .clone();
        let mut captured = initial["auth"].clone();
        captured["tokens"]["refresh_token"] = json!("secret-after-lock");
        captured["last_refresh"] = json!(chrono::Utc::now().to_rfc3339());
        fs::write(
            launch.join("auth.json"),
            serde_json::to_vec(&captured).unwrap(),
        )
        .unwrap();
        fs::write(
            launch.join(".auth-source"),
            format!(
                "{:x}",
                Sha256::digest(initial["auth"].to_string().as_bytes())
            ),
        )
        .unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(home.join(".cc-switch/state-mutation.lock"))
            .unwrap();
        lock.lock().unwrap();
        let mut child = command(home, &capture_args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut blocked = false;
        while std::time::Instant::now() < deadline {
            let wait =
                fs::read_to_string(format!("/proc/{}/wchan", child.id())).unwrap_or_default();
            if wait.contains("locks_lock") {
                blocked = true;
                break;
            }
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let unchanged =
            db.get_all_providers("codex").unwrap()["official"].settings_config == initial;
        lock.unlock().unwrap();
        if !blocked {
            let _ = child.kill();
        }
        let output = child.wait_with_output().unwrap();
        assert!(blocked, "capture did not reach the shared lock");
        assert!(
            unchanged,
            "capture wrote the provider before acquiring the shared lock"
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let live: Value =
            serde_json::from_slice(&fs::read(home.join(".codex/auth.json")).unwrap()).unwrap();
        assert_eq!(live["tokens"]["refresh_token"], "secret-after-lock");
    }
}
