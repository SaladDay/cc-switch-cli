use serial_test::serial;
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

fn assert_success(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout
}

#[test]
#[serial]
fn explicit_config_views_show_complete_credentials() {
    let _lock = lock_test_mutex();
    reset_test_fs();
    let home = ensure_test_home();

    let provider_key = "sk-provider-plaintext-123456";
    let added = run_cc_switch(
        home,
        &[
            "provider",
            "add",
            "--name",
            "Plaintext Provider",
            "--id",
            "plaintext-provider",
            "--base-url",
            "https://api.example.com",
            "--api-key",
            provider_key,
            "--model",
            "claude-sonnet-4-5",
        ],
    );
    assert!(assert_success(&added).contains(provider_key));

    assert_success(&run_cc_switch(
        home,
        &["provider", "switch", "plaintext-provider"],
    ));
    let current = assert_success(&run_cc_switch(home, &["provider", "current"]));
    assert!(current.contains(provider_key), "{current}");

    let preferred_claude_key = "sk-claude-selected-plaintext-123456";
    let stale_claude_key = "sk-claude-stale-plaintext-123456";
    let claude_raw_config = format!(
        r#"{{"env":{{"ANTHROPIC_AUTH_TOKEN":"{stale_claude_key}","ANTHROPIC_API_KEY":"{preferred_claude_key}","ANTHROPIC_BASE_URL":"https://claude-fields.example.com"}}}}"#
    );
    let added = run_cc_switch(
        home,
        &[
            "provider",
            "add",
            "--name",
            "Claude Preferred Field",
            "--id",
            "claude-preferred-field",
            "--config",
            &claude_raw_config,
            "--api-key-field",
            "api-key",
        ],
    );
    let added = assert_success(&added);
    assert!(added.contains(preferred_claude_key), "{added}");
    assert!(!added.contains(stale_claude_key), "{added}");

    assert_success(&run_cc_switch(
        home,
        &["provider", "switch", "claude-preferred-field"],
    ));
    let current = assert_success(&run_cc_switch(home, &["provider", "current"]));
    assert!(current.contains(preferred_claude_key), "{current}");
    assert!(!current.contains(stale_claude_key), "{current}");

    let codex_key = "sk-codex-plaintext-123456";
    let added = run_cc_switch(
        home,
        &[
            "--app",
            "codex",
            "provider",
            "add",
            "--name",
            "Codex Plaintext",
            "--id",
            "codex-plaintext",
            "--base-url",
            "https://codex.example.com/v1",
            "--api-key",
            codex_key,
            "--model",
            "gpt-5.2-codex",
        ],
    );
    assert!(assert_success(&added).contains(codex_key));

    assert_success(&run_cc_switch(
        home,
        &["--app", "codex", "provider", "switch", "codex-plaintext"],
    ));
    let current = assert_success(&run_cc_switch(
        home,
        &["--app", "codex", "provider", "current"],
    ));
    assert!(current.contains(codex_key), "{current}");

    let codex_env_key = "sk-codex-env-plaintext-123456";
    let codex_env_config = serde_json::json!({
        "env": {"OPENAI_API_KEY": codex_env_key},
        "auth": {},
        "config": r#"model_provider = "custom"
model = "gpt-5.2-codex"

[model_providers.custom]
name = "Custom"
base_url = "https://codex-env.example.com/v1"
wire_api = "responses"
requires_openai_auth = true
"#
    })
    .to_string();
    let added = run_cc_switch(
        home,
        &[
            "--app",
            "codex",
            "provider",
            "add",
            "--name",
            "Codex Env Plaintext",
            "--id",
            "codex-env-plaintext",
            "--config",
            &codex_env_config,
        ],
    );
    assert!(assert_success(&added).contains(codex_env_key));

    assert_success(&run_cc_switch(
        home,
        &[
            "--app",
            "codex",
            "provider",
            "switch",
            "codex-env-plaintext",
        ],
    ));
    let current = assert_success(&run_cc_switch(
        home,
        &["--app", "codex", "provider", "current"],
    ));
    assert!(current.contains(codex_env_key), "{current}");

    let codex_toml_key = "sk-codex-toml-plaintext-123456";
    let codex_toml_config = serde_json::json!({
        "auth": {},
        "config": format!(
            r#"model_provider = "custom"
model = "gpt-5.2-codex"

[model_providers.custom]
name = "Custom"
base_url = "https://codex-toml.example.com/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "{codex_toml_key}"
"#
        )
    })
    .to_string();
    let added = run_cc_switch(
        home,
        &[
            "--app",
            "codex",
            "provider",
            "add",
            "--name",
            "Codex TOML Plaintext",
            "--id",
            "codex-toml-plaintext",
            "--config",
            &codex_toml_config,
        ],
    );
    assert!(assert_success(&added).contains(codex_toml_key));

    assert_success(&run_cc_switch(
        home,
        &[
            "--app",
            "codex",
            "provider",
            "switch",
            "codex-toml-plaintext",
        ],
    ));
    let current = assert_success(&run_cc_switch(
        home,
        &["--app", "codex", "provider", "current"],
    ));
    assert!(current.contains(codex_toml_key), "{current}");

    let usage_key = "sk-usage-plaintext-123456";
    assert_success(&run_cc_switch(
        home,
        &[
            "provider",
            "usage-query",
            "set",
            "plaintext-provider",
            "--enabled",
            "--template",
            "general",
            "--api-key",
            usage_key,
            "--base-url",
            "https://usage.example.com",
        ],
    ));
    let usage = assert_success(&run_cc_switch(
        home,
        &["provider", "usage-query", "show", "plaintext-provider"],
    ));
    assert!(usage.contains(usage_key), "{usage}");

    let access_token = "usage-access-token-plaintext-123456";
    assert_success(&run_cc_switch(
        home,
        &[
            "provider",
            "usage-query",
            "set",
            "plaintext-provider",
            "--enabled",
            "--template",
            "newapi",
            "--access-token",
            access_token,
            "--user-id",
            "42",
            "--base-url",
            "https://newapi.example.com",
        ],
    ));
    let usage = assert_success(&run_cc_switch(
        home,
        &["provider", "usage-query", "show", "plaintext-provider"],
    ));
    assert!(usage.contains(access_token), "{usage}");

    let webdav_password = "webdav-password-plaintext";
    assert_success(&run_cc_switch(
        home,
        &[
            "config",
            "webdav",
            "set",
            "--base-url",
            "https://dav.example.com/files",
            "--username",
            "user@example.com",
            "--password",
            webdav_password,
            "--enable",
        ],
    ));
    let webdav = assert_success(&run_cc_switch(home, &["config", "webdav", "show"]));
    assert!(webdav.contains(webdav_password), "{webdav}");
}
