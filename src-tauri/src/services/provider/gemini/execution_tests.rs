//! Execution baselines from CLI dc0b6ced. These describe existing behavior,
//! including incomplete recovery; they are not Core's target safety contract.

use super::*;
use crate::{
    app_config::{McpApps, McpServer},
    config::get_app_config_dir,
    gemini_config::{get_gemini_env_path, get_gemini_settings_path},
    settings,
    test_support::TestEnvGuard,
};
use std::{collections::HashMap, fs};
use tempfile::TempDir;

const OLD_ENV: &str = "# original layout\nGEMINI_API_KEY=old-fake\nOLD=keep\n";
const OLD_SETTINGS: &str =
    "{\"security\":{\"auth\":{\"selectedType\":\"before\"}},\"advanced\":{\"opaque\":true}}\n";

fn seed_native() {
    fs::create_dir_all(get_gemini_env_path().parent().unwrap()).unwrap();
    fs::write(get_gemini_env_path(), OLD_ENV).unwrap();
    fs::write(get_gemini_settings_path(), OLD_SETTINGS).unwrap();
    settings::ensure_security_auth_selected_type("before").unwrap();
}

fn selected_type() -> Option<String> {
    settings::get_settings()
        .security
        .and_then(|security| security.auth)
        .and_then(|auth| auth.selected_type)
}

fn new_provider() -> Provider {
    Provider::with_id(
        "new".into(),
        "Custom".into(),
        json!({"env":{"GEMINI_API_KEY":"new-fake"},"config":{"theme":"dark"}}),
        None,
    )
}

fn prepare() -> PreparedLiveWrite {
    ProviderService::prepare_gemini_live_write(&new_provider(), None, None, true).unwrap()
}

fn state() -> AppState {
    let mut config = MultiAppConfig::default();
    let manager = config.get_manager_mut(&AppType::Gemini).unwrap();
    manager.current = "old".into();
    manager.providers.insert(
        "old".into(),
        Provider::with_id(
            "old".into(),
            "Old".into(),
            json!({"env":{"GEMINI_API_KEY":"old-fake"}}),
            None,
        ),
    );
    manager.providers.insert("new".into(), new_provider());
    super::super::state_from_config(config)
}

fn assert_old_selection(state: &AppState) {
    assert_eq!(
        state.db.get_current_provider("gemini").unwrap().as_deref(),
        Some("old")
    );
    assert_eq!(
        state
            .config
            .read()
            .unwrap()
            .get_manager(&AppType::Gemini)
            .unwrap()
            .current,
        "old"
    );
}

fn assert_restored_native_values() {
    // Existing compensation restores parsed values, not the original layout.
    assert_eq!(
        fs::read_to_string(get_gemini_env_path()).unwrap(),
        "GEMINI_API_KEY=old-fake\nOLD=keep"
    );
    assert_eq!(
        read_json_file::<Value>(&get_gemini_settings_path()).unwrap(),
        serde_json::from_str::<Value>(OLD_SETTINGS).unwrap()
    );
}

#[test]
fn gemini_execution_baseline_stops_in_env_settings_host_flag_order() {
    for blocked in 0..3 {
        let temp = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(temp.path());
        seed_native();
        let prepared = prepare();
        let paths = [
            get_gemini_env_path(),
            get_gemini_settings_path(),
            get_app_config_dir().join("settings.json"),
        ];
        fs::rename(&paths[blocked], temp.path().join("replaced-fixture")).unwrap();
        fs::create_dir(&paths[blocked]).unwrap();

        let error = ProviderService::apply_gemini_live_write(&prepared).unwrap_err();
        assert!(matches!(
            error,
            AppError::IoContext { .. } | AppError::Io { .. }
        ));
        assert!(error.to_string().contains(paths[blocked].to_str().unwrap()));
        assert!(paths[blocked].is_dir());
        if blocked > 0 {
            assert_eq!(
                fs::read_to_string(&paths[0]).unwrap(),
                "GEMINI_API_KEY=new-fake"
            );
        }
        if blocked == 0 {
            assert_eq!(fs::read_to_string(&paths[1]).unwrap(), OLD_SETTINGS);
        } else if blocked == 2 {
            assert_eq!(read_json_file::<Value>(&paths[1]).unwrap()["theme"], "dark");
        }
        assert_eq!(selected_type().as_deref(), Some("before"));
    }
}

#[test]
fn gemini_execution_baseline_force_write_and_switch_have_different_read_requirements() {
    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    seed_native();
    let state = state();
    fs::write(get_gemini_env_path(), [0xff, 0x00]).unwrap();

    let error = ProviderService::switch(&state, AppType::Gemini, "new").unwrap_err();
    assert!(matches!(error, AppError::Io { .. }));
    assert_eq!(fs::read(get_gemini_env_path()).unwrap(), [0xff, 0x00]);
    assert_eq!(
        fs::read_to_string(get_gemini_settings_path()).unwrap(),
        OLD_SETTINGS
    );
    assert_old_selection(&state);

    ProviderService::write_gemini_live_force(&new_provider(), None).unwrap();
    assert_eq!(
        fs::read_to_string(get_gemini_env_path()).unwrap(),
        "GEMINI_API_KEY=new-fake"
    );
    assert_old_selection(&state);
    assert_eq!(selected_type().as_deref(), Some("gemini-api-key"));
}

#[test]
fn gemini_execution_baseline_does_not_detect_changes_after_preparation_or_before_restore() {
    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    seed_native();
    let backup = ProviderService::capture_live_snapshot(&AppType::Gemini).unwrap();
    let prepared = prepare();
    fs::write(get_gemini_env_path(), "EXTERNAL=before-write").unwrap();
    fs::write(get_gemini_settings_path(), r#"{"external":"before-write"}"#).unwrap();
    ProviderService::apply_gemini_live_write(&prepared).unwrap();
    assert_eq!(
        fs::read_to_string(get_gemini_env_path()).unwrap(),
        "GEMINI_API_KEY=new-fake"
    );
    assert!(read_json_file::<Value>(&get_gemini_settings_path())
        .unwrap()
        .get("external")
        .is_none());

    fs::write(get_gemini_env_path(), "EXTERNAL=before-restore").unwrap();
    fs::write(
        get_gemini_settings_path(),
        r#"{"external":"before-restore"}"#,
    )
    .unwrap();
    backup.restore().unwrap();
    assert_restored_native_values();
    assert_eq!(selected_type().as_deref(), Some("gemini-api-key"));
}

#[cfg(unix)]
#[test]
fn gemini_execution_baseline_replaces_leaf_links_and_unreadable_env_without_reading_them() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    seed_native();
    for path in [get_gemini_env_path(), get_gemini_settings_path()] {
        let source = temp.path().join(path.file_name().unwrap());
        fs::rename(&path, &source).unwrap();
        symlink(&source, &path).unwrap();
    }
    let prepared = prepare();
    ProviderService::apply_gemini_live_write(&prepared).unwrap();
    for path in [get_gemini_env_path(), get_gemini_settings_path()] {
        assert!(!fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    }
    assert_eq!(
        fs::read_to_string(temp.path().join(".env")).unwrap(),
        OLD_ENV
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("settings.json")).unwrap(),
        OLD_SETTINGS
    );

    fs::set_permissions(get_gemini_env_path(), fs::Permissions::from_mode(0o000)).unwrap();
    let read_result = fs::read(get_gemini_env_path());
    let write_result = ProviderService::write_gemini_live_force(&new_provider(), None);
    let mode = fs::metadata(get_gemini_env_path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    // Restore access even if a future implementation fails before replacing it.
    fs::set_permissions(get_gemini_env_path(), fs::Permissions::from_mode(0o600)).unwrap();
    // Privileged test users can read mode 000. The separate non-UTF-8 case
    // checks that replacement does not require valid old env text.
    if let Err(error) = read_result {
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }
    write_result.unwrap();
    assert_eq!(mode, 0o600);
    assert_eq!(
        fs::metadata(get_gemini_env_path().parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn gemini_execution_baseline_switch_compensates_a_host_flag_write_failure() {
    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    seed_native();
    let state = state();
    let host_settings = get_app_config_dir().join("settings.json");
    fs::rename(&host_settings, temp.path().join("host-settings-before")).unwrap();
    fs::create_dir(&host_settings).unwrap();

    let error = ProviderService::switch(&state, AppType::Gemini, "new").unwrap_err();
    assert!(error.to_string().contains(host_settings.to_str().unwrap()));
    assert_old_selection(&state);
    assert_restored_native_values();
    assert_eq!(selected_type().as_deref(), Some("before"));
}

#[test]
fn gemini_execution_baseline_later_mcp_failure_restores_native_but_not_host_auth_flag() {
    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    seed_native();
    let state = state();
    let broken = McpServer {
        id: "fixture-tool".into(),
        name: "Fixture".into(),
        server: json!({"command":"fixture-not-executed"}),
        apps: McpApps {
            codex: true,
            ..Default::default()
        },
        description: None,
        homepage: None,
        docs: None,
        tags: vec![],
    };
    state.db.save_mcp_server(&broken).unwrap();
    state.config.write().unwrap().mcp.servers = Some(HashMap::from([(broken.id.clone(), broken)]));
    let codex_path = get_codex_config_path();
    fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
    fs::write(&codex_path, "[invalid").unwrap();

    let error = ProviderService::switch(&state, AppType::Gemini, "new").unwrap_err();
    assert!(error.to_string().contains("MCP"));
    assert!(error.to_string().contains("codex"));
    assert_old_selection(&state);
    assert_restored_native_values();
    assert_eq!(selected_type().as_deref(), Some("gemini-api-key"));
    assert_eq!(fs::read_to_string(codex_path).unwrap(), "[invalid");
}
