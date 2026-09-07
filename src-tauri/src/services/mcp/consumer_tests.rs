use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use serde_json::{json, Value};

use super::*;
use crate::{database::Database, test_support::TestEnvGuard};

struct LitePeer(Child);

impl LitePeer {
    fn create_mcp(home: &Path) {
        let binary = std::env::var_os("CC_SWITCH_LITE_TEST_BINARY")
            .expect("set CC_SWITCH_LITE_TEST_BINARY to the built Lite library test binary");
        let mut peer = Self(
            Command::new(binary)
                .args([
                    "--ignored",
                    "--exact",
                    "consumer_coordination::create_mcp_in_cli_fixture",
                    "--test-threads=1",
                ])
                .env("CC_SWITCH_COORDINATION_HOME", home)
                .stdin(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = peer.0.try_wait().unwrap() {
                assert!(status.success(), "Lite MCP peer failed: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "Lite MCP peer did not exit");
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

#[test]
#[ignore = "requires the independently built Lite library test binary"]
fn gemini_toggle_preserves_mcp_created_by_lite_after_cli_startup() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    let settings = crate::gemini_config::get_gemini_settings_path();
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let native = json!({
        "fixture": {"keep": true},
        "mcpServers": {"native-peer": {"command": "not-executed-native-peer", "timeout": 60000}}
    });
    fs::write(&settings, serde_json::to_vec(&native).unwrap()).unwrap();
    let db = Arc::new(Database::init().unwrap());
    let target = McpServer {
        id: "cli-target".into(),
        name: "CLI target fixture".into(),
        server: json!({"command":"not-executed-cli-target", "timeout":60000}),
        apps: McpApps::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: vec![],
    };
    db.save_mcp_server(&target).unwrap();
    fs::write(temp.path().join("coordination-fixture"), "cli-lite-v1").unwrap();
    let state = AppState::new(db);
    LitePeer::create_mcp(temp.path());

    // Prove the peer committed after the CLI snapshot was constructed.
    assert!(!state
        .config
        .read()
        .unwrap()
        .mcp
        .servers
        .as_ref()
        .unwrap()
        .contains_key("lite-peer"));
    let before = state.db.get_all_mcp_servers().unwrap();
    let peer = before
        .get("lite-peer")
        .expect("Lite must commit its record");
    let peer_before = serde_json::to_value(peer).unwrap();
    assert!(!before[&target.id].apps.gemini);

    McpService::toggle_app(&state, &target.id, AppType::Gemini, true).unwrap();

    let after = state.db.get_all_mcp_servers().unwrap();
    assert!(after[&target.id].apps.gemini);
    let written: Value = serde_json::from_slice(&fs::read(settings).unwrap()).unwrap();
    assert_eq!(written["fixture"], native["fixture"]);
    assert_eq!(written["mcpServers"][&target.id], target.server);
    assert_eq!(
        written["mcpServers"]["native-peer"],
        native["mcpServers"]["native-peer"]
    );
    assert_eq!(
        after
            .get("lite-peer")
            .map(|server| serde_json::to_value(server).unwrap()),
        Some(peer_before),
        "CLI MCP toggle must preserve Lite's independently committed record"
    );
}
