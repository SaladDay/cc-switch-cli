use std::fs;

use serde_json::json;

use cc_switch_lib::{get_claude_mcp_path, get_skip_claude_onboarding, set_skip_claude_onboarding};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs};

#[test]
fn skip_claude_onboarding_setting_writes_and_clears_has_completed_onboarding() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let mcp_path = get_claude_mcp_path();
    let initial = json!({
        "mcpServers": {
            "echo": {
                "type": "stdio",
                "command": "echo"
            }
        },
        "other": 1
    });
    fs::write(
        &mcp_path,
        serde_json::to_string_pretty(&initial).expect("serialize seed json"),
    )
    .expect("seed ~/.claude.json");

    set_skip_claude_onboarding(true).expect("enable skip onboarding");
    assert!(
        get_skip_claude_onboarding(),
        "settings flag should be true after enabling"
    );

    let content = fs::read_to_string(&mcp_path).expect("read ~/.claude.json after enabling");
    let value: serde_json::Value = serde_json::from_str(&content).expect("parse updated json");
    assert_eq!(value["hasCompletedOnboarding"], json!(true));
    assert_eq!(value["mcpServers"]["echo"]["command"], json!("echo"));
    assert_eq!(value["other"], json!(1));

    set_skip_claude_onboarding(false).expect("disable skip onboarding");
    assert!(
        !get_skip_claude_onboarding(),
        "settings flag should be false after disabling"
    );

    let content = fs::read_to_string(&mcp_path).expect("read ~/.claude.json after disabling");
    let value: serde_json::Value = serde_json::from_str(&content).expect("parse updated json");
    assert!(
        value.get("hasCompletedOnboarding").is_none(),
        "field should be removed when disabling skip onboarding"
    );
    assert_eq!(value["mcpServers"]["echo"]["command"], json!("echo"));
    assert_eq!(value["other"], json!(1));
}
