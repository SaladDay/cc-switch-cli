#![cfg(all(unix, feature = "cli"))]

mod support;

use cc_switch_lib::{AppState, Provider};
use serde_json::json;
use std::fs;
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Running(Option<Child>);

impl Running {
    fn finish(mut self) -> Output {
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
        }
    }
}

struct Fixture {
    root: PathBuf,
    work: PathBuf,
    state: AppState,
}

impl Fixture {
    fn new() -> Self {
        support::reset_test_fs();
        let root = support::ensure_test_home().to_path_buf();
        let state = AppState::try_new().unwrap();
        for id in ["team", "plus"] {
            let mut provider = Provider::with_id(
                id.into(),
                id.into(),
                json!({
                    "config": "model = 'original-model'\n", "auth": {"OPENAI_API_KEY": format!("original-{id}")}
                }),
                None,
            );
            provider.category = Some("official".into());
            state.db.save_provider("codex", &provider).unwrap();
        }
        let work = root.join(".config/shared-launch-test");
        fs::create_dir_all(work.join("bin")).unwrap();
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::write(root.join(".codex/config.toml"), "model = 'global-model'\n").unwrap();
        fs::write(
            root.join(".codex/auth.json"),
            "{\"OPENAI_API_KEY\":\"global-key\"}",
        )
        .unwrap();
        let stub = work.join("bin/codex");
        fs::write(
            &stub,
            r#"#!/bin/sh
trap 'exit 130' INT TERM HUP
mkdir -p "$CODEX_HOME/sessions"
printf '%s\n' "$FAKE_LABEL" > "$CODEX_HOME/sessions/$FAKE_LABEL.jsonl"
if [ -f "$CODEX_HOME/auth.json" ]; then cp "$CODEX_HOME/auth.json" "$FAKE_OUTPUT.auth-before"; fi
printf '{"OPENAI_API_KEY":"refreshed-%s"}\n' "$FAKE_LABEL" > "$CODEX_HOME/auth.json"
# Native deletion can atomically replace this local title cache.
printf '{}\n' > "$CODEX_HOME/session_index.jsonl.tmp"
mv "$CODEX_HOME/session_index.jsonl.tmp" "$CODEX_HOME/session_index.jsonl"
printf '%s' "$CODEX_HOME" > "$FAKE_OUTPUT"
while [ ! -f "$FAKE_GATE" ]; do sleep 0.05; done
if [ -n "$FAKE_BACKGROUND_PID" ]; then
    sleep 30 </dev/null >/dev/null 2>&1 &
    printf '%s' "$!" > "$FAKE_BACKGROUND_PID"
fi
exit 0
"#,
        )
        .unwrap();
        fs::set_permissions(stub, fs::Permissions::from_mode(0o755)).unwrap();
        Self { root, work, state }
    }

    fn command(&self, id: &str, output: &str, shared: bool) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cc-switch"));
        command.args(["start", "codex", id]);
        if shared {
            command.arg("--shared-sessions");
        }
        command
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.work.join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("FAKE_LABEL", id)
            .env("FAKE_OUTPUT", self.work.join(output))
            .env("FAKE_GATE", self.work.join("gate"))
            .env_remove("CC_SWITCH_DAEMON_SOCKET")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        command
    }

    fn start(&self, id: &str, output: &str, shared: bool) -> Running {
        Running(Some(self.command(id, output, shared).spawn().unwrap()))
    }

    fn started_home(&self, output: &str) -> PathBuf {
        let path = self.work.join(output);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(text) = fs::read_to_string(&path) {
                if !text.is_empty() {
                    return PathBuf::from(text);
                }
            }
            assert!(
                Instant::now() < deadline,
                "Codex stub did not start: {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn shared_start_keeps_two_providers_isolated_and_history_resumable_after_exit() {
    let _guard = support::lock_test_mutex();
    let fixture = Fixture::new();
    let a = fixture.start("team", "a", true);
    let a_home = fixture.started_home("a");
    let b = fixture.start("plus", "b", true);
    let b_home = fixture.started_home("b");
    assert_ne!(a_home, b_home);
    assert_eq!(
        fs::read_to_string(b_home.join("sessions/team.jsonl")).unwrap(),
        "team\n"
    );
    assert_eq!(
        fs::read_to_string(a_home.join("sessions/plus.jsonl")).unwrap(),
        "plus\n"
    );
    let duplicate = fixture.command("team", "duplicate", true).output().unwrap();
    assert!(!duplicate.status.success());
    assert!(!fixture.work.join("duplicate").exists());

    // An edit made while both sessions run must survive the exit-time auth capture.
    let mut edited = fixture
        .state
        .db
        .get_provider_by_id("team", "codex")
        .unwrap()
        .unwrap();
    edited.settings_config["config"] = json!("model = 'edited-while-running'\n");
    fixture.state.db.save_provider("codex", &edited).unwrap();
    fs::write(fixture.work.join("gate"), "exit both").unwrap();
    success(a.finish());
    success(b.finish());
    for (id, home) in [("team", &a_home), ("plus", &b_home)] {
        let saved = fixture
            .state
            .db
            .get_provider_by_id(id, "codex")
            .unwrap()
            .unwrap();
        assert_eq!(
            saved.settings_config["auth"]["OPENAI_API_KEY"],
            format!("refreshed-{id}")
        );
        let expected = if id == "team" {
            "model = 'edited-while-running'\n"
        } else {
            "model = 'original-model'\n"
        };
        assert_eq!(saved.settings_config["config"], expected);
        assert!(
            home.join("sessions/team.jsonl").exists(),
            "stored rollout paths must survive exit"
        );
        let config: toml::Value = fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(config["model_provider"].as_str(), Some("custom"));
        assert_eq!(
            Path::new(config["sqlite_home"].as_str().unwrap()),
            fixture.root.join(".codex")
        );
    }
    assert_eq!(
        fs::read_to_string(fixture.root.join(".codex/config.toml")).unwrap(),
        "model = 'global-model'\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join(".codex/auth.json")).unwrap(),
        "{\"OPENAI_API_KEY\":\"global-key\"}"
    );
    success(fixture.command("team", "again", true).output().unwrap());
    assert_eq!(fixture.started_home("again"), a_home);
    assert!(a_home.join("sessions/plus.jsonl").exists());
}

#[test]
fn shared_start_dry_run_and_override_errors_do_not_create_launch_homes() {
    let _guard = support::lock_test_mutex();
    let fixture = Fixture::new();
    let output = fixture
        .command("team", "unused", true)
        .arg("--dry-run")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("CODEX_HOME="));
    assert!(!stdout.contains("original-team"));
    assert!(!fixture.root.join(".codex/.cc-switch-launches").exists());
    let output = fixture
        .command("team", "unused", true)
        .args(["--", "-c", "model_provider='other'"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!fixture.root.join(".codex/.cc-switch-launches").exists());
}

#[test]
fn shared_start_resolves_relative_sqlite_config_from_the_source_home() {
    let _guard = support::lock_test_mutex();
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join(".codex/config.toml"),
        "sqlite_home = 'relative-state'\n",
    )
    .unwrap();
    fs::write(fixture.work.join("gate"), "exit").unwrap();
    for (id, cwd) in [("team", &fixture.root), ("plus", &fixture.work)] {
        success(
            fixture
                .command(id, id, true)
                .current_dir(cwd)
                .output()
                .unwrap(),
        );
        let config: toml::Value = fs::read_to_string(fixture.started_home(id).join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            Path::new(config["sqlite_home"].as_str().unwrap()),
            fixture.root.join(".codex/relative-state")
        );
    }
}

#[test]
fn shared_start_retains_refresh_after_kill_but_honors_explicit_credential_edits() {
    let _guard = support::lock_test_mutex();
    let fixture = Fixture::new();
    let running = fixture.start("team", "killed", true);
    fixture.started_home("killed");
    unsafe { libc::kill(-(running.0.as_ref().unwrap().id() as i32), libc::SIGKILL) };
    assert!(!running.finish().status.success());
    fs::write(fixture.work.join("gate"), "exit").unwrap();
    success(fixture.command("team", "recovered", true).output().unwrap());
    let before: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(fixture.work.join("recovered.auth-before")).unwrap(),
    )
    .unwrap();
    assert_eq!(before["OPENAI_API_KEY"], "refreshed-team");

    let mut edited = fixture
        .state
        .db
        .get_provider_by_id("team", "codex")
        .unwrap()
        .unwrap();
    edited.settings_config["auth"] = json!({"OPENAI_API_KEY": "explicit-edit"});
    fixture.state.db.save_provider("codex", &edited).unwrap();
    success(fixture.command("team", "edited", true).output().unwrap());
    let before: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fixture.work.join("edited.auth-before")).unwrap())
            .unwrap();
    assert_eq!(before["OPENAI_API_KEY"], "explicit-edit");
}

#[test]
fn shared_start_exit_preserves_credentials_edited_while_codex_runs() {
    let _guard = support::lock_test_mutex();
    let fixture = Fixture::new();
    for (id, changed_in_codex) in [("team", false), ("plus", true)] {
        let running = fixture.start(id, id, true);
        let home = fixture.started_home(id);
        if !changed_in_codex {
            fs::write(
                home.join("auth.json"),
                format!(r#"{{"OPENAI_API_KEY":"original-{id}"}}"#),
            )
            .unwrap();
        }
        let mut edited = fixture
            .state
            .db
            .get_provider_by_id(id, "codex")
            .unwrap()
            .unwrap();
        edited.settings_config["auth"] = json!({"OPENAI_API_KEY": "edited-while-running"});
        fixture.state.db.save_provider("codex", &edited).unwrap();
        fs::write(fixture.work.join("gate"), "exit").unwrap();
        success(running.finish());
        fs::remove_file(fixture.work.join("gate")).unwrap();
        assert_eq!(
            fixture
                .state
                .db
                .get_provider_by_id(id, "codex")
                .unwrap()
                .unwrap()
                .settings_config["auth"]["OPENAI_API_KEY"],
            "edited-while-running"
        );
    }
}

#[test]
fn shared_start_interrupt_retains_history_and_releases_provider_lock() {
    let _guard = support::lock_test_mutex();
    let fixture = Fixture::new();
    let running = fixture.start("team", "before-interrupt", true);
    let home = fixture.started_home("before-interrupt");
    unsafe {
        libc::kill(-(running.0.as_ref().unwrap().id() as i32), libc::SIGINT);
    }
    assert!(!running.finish().status.success());
    assert!(home.join("sessions/team.jsonl").exists());
    fs::write(fixture.work.join("gate"), "exit").unwrap();
    success(
        fixture
            .command("team", "after-interrupt", true)
            .output()
            .unwrap(),
    );
    assert_eq!(fixture.started_home("after-interrupt"), home);
}

#[test]
fn shared_start_keeps_home_locked_when_only_the_supervisor_is_killed() {
    let _guard = support::lock_test_mutex();
    let fixture = Fixture::new();
    let mut running = fixture.start("team", "orphan", true);
    let home = fixture.started_home("orphan");
    let supervisor = running.0.as_mut().unwrap();
    unsafe { libc::kill(supervisor.id() as i32, libc::SIGKILL) };
    assert!(!supervisor.wait().unwrap().success());
    let duplicate = fixture.command("team", "duplicate", true).output().unwrap();
    assert!(!duplicate.status.success());
    assert!(!fixture.work.join("duplicate").exists());
    assert!(fs::read_to_string(home.join("auth.json"))
        .unwrap()
        .contains("refreshed-team"));
    fs::write(fixture.work.join("gate"), "exit surviving Codex").unwrap();
    assert!(!running.finish().status.success());
    success(
        fixture
            .command("team", "after-orphan", true)
            .output()
            .unwrap(),
    );
}

#[test]
fn shared_start_releases_lock_while_native_background_child_is_alive() {
    let _guard = support::lock_test_mutex();
    let fixture = Fixture::new();
    fs::write(fixture.work.join("gate"), "exit").unwrap();
    let pid_path = fixture.work.join("background-pid");
    let output = fixture
        .command("team", "first", true)
        .env("FAKE_BACKGROUND_PID", &pid_path)
        .output()
        .unwrap();
    struct Background(i32);
    impl Drop for Background {
        fn drop(&mut self) {
            unsafe { libc::kill(self.0, libc::SIGKILL) };
        }
    }
    let background = Background(fs::read_to_string(pid_path).unwrap().parse().unwrap());
    success(output);
    assert_eq!(unsafe { libc::kill(background.0, 0) }, 0);
    success(fixture.command("team", "second", true).output().unwrap());
    assert_eq!(unsafe { libc::kill(background.0, 0) }, 0);
}

#[test]
fn ordinary_start_still_uses_and_removes_a_temporary_home() {
    let _guard = support::lock_test_mutex();
    let fixture = Fixture::new();
    fs::write(fixture.work.join("gate"), "exit").unwrap();
    success(
        fixture
            .command("team", "ordinary", false)
            .args(["--", "--model", "another-model"])
            .output()
            .unwrap(),
    );
    let home = fixture.started_home("ordinary");
    assert!(home.starts_with(std::env::temp_dir()));
    assert!(!home.exists());
    assert!(!fixture.root.join(".codex/.cc-switch-launches").exists());
}
