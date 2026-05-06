use serde_json::json;
use serial_test::serial;

use cc_switch_lib::{
    cli::commands::prompts::{execute, PromptsCommand},
    AppType, MultiAppConfig, PromptService,
};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs, state_from_config};

#[test]
#[serial]
fn prompt_service_rename_updates_name_and_timestamp() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let mut config = MultiAppConfig::default();
    config.prompts.claude.prompts = serde_json::from_value(json!({
        "pr1": {
            "id": "pr1",
            "name": "Old Name",
            "content": "hello",
            "enabled": false,
            "createdAt": 1,
            "updatedAt": 1
        }
    }))
    .expect("deserialize prompts");
    let state = state_from_config(config);

    PromptService::rename_prompt(&state, AppType::Claude, "pr1", "New Name")
        .expect("rename prompt succeeds");

    let prompts = PromptService::get_prompts(&state, AppType::Claude).expect("load prompts");
    let prompt = prompts.get("pr1").expect("renamed prompt should exist");
    assert_eq!(prompt.name, "New Name");
    assert!(prompt.updated_at.unwrap_or_default() >= 1);
}

#[test]
#[serial]
fn prompt_service_rename_rejects_empty_name() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let mut config = MultiAppConfig::default();
    config.prompts.claude.prompts = serde_json::from_value(json!({
        "pr1": {
            "id": "pr1",
            "name": "Old Name",
            "content": "hello",
            "enabled": false,
            "createdAt": 1,
            "updatedAt": 1
        }
    }))
    .expect("deserialize prompts");
    let state = state_from_config(config);

    let err = PromptService::rename_prompt(&state, AppType::Claude, "pr1", "   ")
        .expect_err("empty name should fail");
    assert!(
        err.to_string().contains("不能为空"),
        "unexpected error: {err}"
    );
}

#[test]
#[serial]
fn prompt_rename_command_updates_prompt_name() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let mut config = MultiAppConfig::default();
    config.prompts.claude.prompts = serde_json::from_value(json!({
        "pr1": {
            "id": "pr1",
            "name": "Old Name",
            "content": "hello",
            "enabled": false,
            "createdAt": 1,
            "updatedAt": 1
        }
    }))
    .expect("deserialize prompts");
    let state = state_from_config(config);
    state.save().expect("persist config");

    execute(
        PromptsCommand::Rename {
            id: "pr1".to_string(),
            name: Some("New Name".to_string()),
        },
        Some(AppType::Claude),
    )
    .expect("rename command succeeds");

    let persisted = cc_switch_lib::AppState::try_new().expect("reload state");
    let prompts = PromptService::get_prompts(&persisted, AppType::Claude).expect("load prompts");
    assert_eq!(
        prompts.get("pr1").map(|prompt| prompt.name.as_str()),
        Some("New Name")
    );
}

#[test]
#[serial]
fn prompt_create_command_uses_explicit_name() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    ensure_test_home();

    let state = state_from_config(MultiAppConfig::default());
    state.save().expect("persist config");

    let editor_script = ensure_test_home().join("fake-editor.sh");
    std::fs::write(
        &editor_script,
        "#!/bin/sh\nprintf 'system prompt body\\n' > \"$1\"\n",
    )
    .expect("write fake editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&editor_script)
            .expect("read fake editor metadata")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&editor_script, perms).expect("chmod fake editor");
    }

    std::env::set_var("EDITOR", &editor_script);
    std::env::set_var("VISUAL", &editor_script);

    execute(
        PromptsCommand::Create {
            name: Some("Prompt One".to_string()),
        },
        Some(AppType::Claude),
    )
    .expect("create command succeeds");

    std::env::remove_var("EDITOR");
    std::env::remove_var("VISUAL");

    let persisted = cc_switch_lib::AppState::try_new().expect("reload state");
    let prompts = PromptService::get_prompts(&persisted, AppType::Claude).expect("load prompts");
    let prompt = prompts
        .get("prompt-one")
        .expect("created prompt should exist");
    assert_eq!(prompt.name, "Prompt One");
    assert_eq!(prompt.content, "system prompt body");
}

#[test]
#[serial]
fn generate_prompt_id_falls_back_when_name_has_no_valid_slug_chars() {
    let ids = vec!["prompt".to_string(), "prompt-1".to_string()];
    let generated = PromptService::generate_prompt_id("!!!", &ids);
    assert_eq!(generated, "prompt-2");
}
