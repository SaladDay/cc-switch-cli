use super::*;
use std::{
    io::Write,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct LitePeer(Child);

impl LitePeer {
    fn start(home: &Path, mode: &str) -> Self {
        let binary = std::env::var_os("CC_SWITCH_LITE_TEST_BINARY")
            .expect("set CC_SWITCH_LITE_TEST_BINARY to the built Lite library test binary");
        Self(
            Command::new(binary)
                .args([
                    "--ignored",
                    "--exact",
                    "consumer_coordination::native_switch_in_cli_fixture",
                    "--test-threads=1",
                ])
                .env("CC_SWITCH_COORDINATION_HOME", home)
                .env("CC_SWITCH_COORDINATION_MODE", mode)
                .stdin(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }

    fn finish(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "Lite peer failed: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "Lite peer did not exit");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn run(home: &Path, mode: &str) {
        Self::start(home, mode).finish();
    }

    fn wait_held(&mut self, home: &Path) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !home.join("lite-native-held").is_file() {
            assert!(
                self.0.try_wait().unwrap().is_none(),
                "Lite holder exited early"
            );
            assert!(Instant::now() < deadline, "Lite holder did not signal");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for LitePeer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn shared_state(home: &Path) -> AppState {
    seed_native();
    let db = std::sync::Arc::new(Database::init().unwrap());
    db.save_provider(
        "gemini",
        &Provider::with_id(
            "old".into(),
            "Old".into(),
            json!({"env":{"GEMINI_API_KEY":"old-fake"}}),
            None,
        ),
    )
    .unwrap();
    db.save_provider("gemini", &new_provider()).unwrap();
    db.set_current_provider("gemini", "old").unwrap();
    fs::write(home.join("coordination-fixture"), "cli-lite-v1").unwrap();
    AppState::new(db)
}

fn native_bytes() -> (Vec<u8>, Vec<u8>) {
    (
        fs::read(get_gemini_env_path()).unwrap(),
        fs::read(get_gemini_settings_path()).unwrap(),
    )
}

#[test]
fn gemini_live_lock_is_released_after_validation_errors() {
    use cc_switch_core::fs::{shared_live_config_lock_path, SharedLiveConfigLock};
    for invalid_document in [false, true] {
        let temp = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(temp.path());
        let state = shared_state(temp.path());
        if invalid_document {
            fs::write(get_gemini_settings_path(), "[invalid").unwrap();
        }
        let before = native_bytes();
        let rows =
            cc_switch_store::read_provider_rows(&state.db.conn.lock().unwrap(), None).unwrap();
        let id = if invalid_document { "new" } else { "missing" };
        assert!(ProviderService::switch(&state, AppType::Gemini, id).is_err());
        assert_eq!(native_bytes(), before);
        assert_eq!(
            cc_switch_store::read_provider_rows(&state.db.conn.lock().unwrap(), None).unwrap(),
            rows
        );
        let _lock =
            SharedLiveConfigLock::try_acquire(&shared_live_config_lock_path(temp.path())).unwrap();
    }
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn gemini_switch_rejects_a_live_lock_held_by_lite() {
    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let state = shared_state(temp.path());
    let mut peer = LitePeer::start(temp.path(), "hold");
    peer.wait_held(temp.path());
    let before = native_bytes();
    let rows = cc_switch_store::read_provider_rows(&state.db.conn.lock().unwrap(), None).unwrap();
    assert!(
        matches!(
            ProviderService::switch(&state, AppType::Gemini, "new"),
            Err(AppError::Conflict(_))
        ),
        "CLI must reject a native write while Lite holds the shared lock"
    );
    assert_eq!(native_bytes(), before);
    assert_eq!(
        cc_switch_store::read_provider_rows(&state.db.conn.lock().unwrap(), None).unwrap(),
        rows
    );
    assert_old_selection(&state);
    peer.0.stdin.as_mut().unwrap().write_all(b"r").unwrap();
    peer.finish();
    assert_restored_native_bytes();
    ProviderService::switch(&state, AppType::Gemini, "new").unwrap();
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn gemini_live_lock_spans_publication_and_recovery() {
    for failure in ["none", "native", "mcp", "commit"] {
        let temp = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(temp.path());
        let state = shared_state(temp.path());
        if failure == "mcp" {
            state
                .db
                .save_mcp_server(&McpServer {
                    id: "peer".into(),
                    name: "Peer".into(),
                    server: json!({"command":"not-executed"}),
                    apps: McpApps {
                        gemini: true,
                        codex: true,
                        ..Default::default()
                    },
                    description: None,
                    homepage: None,
                    docs: None,
                    tags: vec![],
                })
                .unwrap();
            fs::create_dir_all(get_codex_config_path().parent().unwrap()).unwrap();
            fs::write(get_codex_config_path(), "[invalid").unwrap();
        }
        if failure == "commit" {
            state.db.conn.lock().unwrap().execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE UNIQUE INDEX fixture_current_reference ON providers(id, app_type, is_current);
                 CREATE TABLE fixture_commit_guard(id TEXT, app_type TEXT, selected INTEGER,
                     FOREIGN KEY(id, app_type, selected) REFERENCES providers(id, app_type, is_current)
                     DEFERRABLE INITIALLY DEFERRED);
                 INSERT INTO fixture_commit_guard VALUES('old', 'gemini', 1);"
            ).unwrap();
        }
        let home = temp.path().to_owned();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = calls.clone();
        let result = crate::gemini_config::operation::with_hook(
            Box::new(move |_, _| {
                let before = native_bytes();
                LitePeer::run(&home, "probe_locked");
                assert_eq!(native_bytes(), before);
                observed.set(observed.get() + 1);
                if failure == "native" && observed.get() == 2 {
                    return Err(AppError::io(
                        get_gemini_settings_path(),
                        std::io::Error::other("fixture publication failure"),
                    ));
                }
                Ok(())
            }),
            || ProviderService::switch(&state, AppType::Gemini, "new"),
        );
        if failure == "none" {
            result.unwrap();
            assert_eq!(
                state.db.get_current_provider("gemini").unwrap().as_deref(),
                Some("new")
            );
        } else {
            assert!(result.is_err(), "{failure} must fail");
            assert!(calls.get() > 2, "must probe native recovery too");
            assert_restored_native_bytes();
            assert_old_selection(&state);
        }
        let before = native_bytes();
        LitePeer::run(temp.path(), "probe_released");
        assert_eq!(native_bytes(), before);
    }
}

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn gemini_real_consumer_switches_share_catalog_and_native_files() {
    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let state = shared_state(temp.path());
    LitePeer::run(temp.path(), "store_switch");
    assert_eq!(
        state.db.get_current_provider("gemini").unwrap().as_deref(),
        Some("new")
    );
    assert_eq!(
        crate::gemini_config::read_gemini_env().unwrap()["GEMINI_API_KEY"],
        "new-fake"
    );
    ProviderService::switch(&state, AppType::Gemini, "old").unwrap();
    assert_old_selection(&state);
    assert_eq!(
        crate::gemini_config::read_gemini_env().unwrap()["GEMINI_API_KEY"],
        "old-fake"
    );
    LitePeer::run(temp.path(), "store_switch");
    assert_eq!(
        state.db.get_current_provider("gemini").unwrap().as_deref(),
        Some("new")
    );
}
