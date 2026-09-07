//! Force-write compatibility baselines and ordinary-switch recovery contracts.

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

fn assert_restored_native_bytes() {
    assert_eq!(fs::read_to_string(get_gemini_env_path()).unwrap(), OLD_ENV);
    assert_eq!(
        fs::read_to_string(get_gemini_settings_path()).unwrap(),
        OLD_SETTINGS
    );
}

#[test]
#[ignore = "requires the Lite library test binary; shared-consumer migration acceptance"]
fn gemini_switch_preserves_a_provider_committed_by_lite_after_cli_observation() {
    let lite_tests = std::env::var_os("CC_SWITCH_LITE_TEST_BINARY")
        .expect("set CC_SWITCH_LITE_TEST_BINARY to the independently built Lite test binary");
    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    seed_native();
    let db = std::sync::Arc::new(crate::Database::init().unwrap());
    let old = Provider::with_id(
        "old".into(),
        "Old".into(),
        json!({"env":{"GEMINI_API_KEY":"old-fake"}}),
        None,
    );
    db.save_provider("gemini", &old).unwrap();
    db.save_provider("gemini", &new_provider()).unwrap();
    db.set_current_provider("gemini", "old").unwrap();
    let state = AppState::new(db);
    // The marker is created only under this test's owned temporary profile.
    fs::write(temp.path().join("coordination-fixture"), "cli-lite-v1").unwrap();
    let mut peer = std::process::Command::new(lite_tests)
        .args([
            "--ignored",
            "--exact",
            "consumer_coordination::create_provider_in_cli_fixture",
            "--test-threads=1",
        ])
        .env("CC_SWITCH_COORDINATION_HOME", temp.path())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        match peer.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            outcome => {
                let _ = peer.kill();
                let _ = peer.wait();
                panic!("Lite fixture writer did not finish: {outcome:?}");
            }
        }
    };
    assert!(status.success(), "Lite fixture writer failed: {status}");
    let id = fs::read_to_string(temp.path().join("lite-provider-id")).unwrap();
    assert!(state
        .db
        .get_provider_by_id(&id, "gemini")
        .unwrap()
        .is_some());

    ProviderService::switch(&state, AppType::Gemini, "new").unwrap();

    assert_eq!(
        state.db.get_current_provider("gemini").unwrap().as_deref(),
        Some("new")
    );
    assert_eq!(
        fs::read_to_string(get_gemini_env_path()).unwrap(),
        "GEMINI_API_KEY=new-fake"
    );
    assert!(
        state
            .db
            .get_provider_by_id(&id, "gemini")
            .unwrap()
            .is_some(),
        "ordinary switching must not remove a provider committed by Lite"
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
fn gemini_switch_compensates_a_host_flag_write_failure_with_original_bytes() {
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
    assert_restored_native_bytes();
    assert_eq!(selected_type().as_deref(), Some("before"));
}

#[test]
fn gemini_switch_later_mcp_failure_restores_bytes_and_keeps_host_auth_policy() {
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
    assert_restored_native_bytes();
    assert_eq!(selected_type().as_deref(), Some("gemini-api-key"));
    assert_eq!(fs::read_to_string(codex_path).unwrap(), "[invalid");
}

fn seed_mcp(state: &AppState) {
    let servers = (0..2)
        .map(|index| {
            let id = format!("fixture-{index}");
            let server = McpServer {
                id: id.clone(),
                name: id.clone(),
                server: json!({"command":"fixture-not-executed"}),
                apps: McpApps {
                    gemini: true,
                    ..Default::default()
                },
                description: None,
                homepage: None,
                docs: None,
                tags: vec![],
            };
            state.db.save_mcp_server(&server).unwrap();
            (id, server)
        })
        .collect();
    state.config.write().unwrap().mcp.servers = Some(servers);
}

#[test]
fn gemini_switch_recovers_each_native_and_mcp_publication_failure() {
    use crate::gemini_config::operation::with_hook;
    for fail_at in 1..=4 {
        for publish_first in [false, true] {
            let temp = TempDir::new().unwrap();
            let _guard = TestEnvGuard::isolated(temp.path());
            seed_native();
            let state = state();
            seed_mcp(&state);
            let mut calls = 0;
            let error = with_hook(
                Box::new(move |resource, replacement| {
                    calls += 1;
                    if calls == fail_at {
                        if publish_first {
                            resource.write(replacement.unwrap())?;
                        }
                        return Err(AppError::io(
                            resource.path(),
                            std::io::Error::other("fixture publication failure"),
                        ));
                    }
                    Ok(())
                }),
                || ProviderService::switch(&state, AppType::Gemini, "new"),
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("fixture publication failure"),
                "{error}"
            );
            assert_restored_native_bytes();
            assert_old_selection(&state);
            assert_eq!(
                selected_type().as_deref(),
                Some(if fail_at < 3 {
                    "before"
                } else {
                    "gemini-api-key"
                })
            );
        }
    }
}

#[test]
fn gemini_switch_keeps_external_settings_and_does_not_restore_dependent_env() {
    use crate::gemini_config::operation::with_hook;
    for edit_at in [2, 4] {
        let temp = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(temp.path());
        seed_native();
        let state = state();
        seed_mcp(&state);
        let mut calls = 0;
        let error = with_hook(
            Box::new(move |resource, _| {
                calls += 1;
                if calls == edit_at {
                    fs::write(resource.path(), "{\"external\":true}").unwrap();
                }
                Ok(())
            }),
            || ProviderService::switch(&state, AppType::Gemini, "new"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Gemini"), "{error}");
        assert_eq!(
            fs::read_to_string(get_gemini_settings_path()).unwrap(),
            "{\"external\":true}"
        );
        assert_eq!(
            fs::read_to_string(get_gemini_env_path()).unwrap(),
            "GEMINI_API_KEY=new-fake"
        );
        assert_old_selection(&state);
    }
}

#[test]
fn gemini_switch_supports_large_native_documents_and_missing_env_recovery() {
    use crate::gemini_config::operation::with_hook;
    for missing_env in [false, true] {
        let temp = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(temp.path());
        seed_native();
        let large = format!(
            "{{\"opaque\":\"{}\"}}\n",
            "x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES + 1)
        );
        fs::write(get_gemini_settings_path(), &large).unwrap();
        if missing_env {
            fs::remove_file(get_gemini_env_path()).unwrap();
        }
        let state = state();
        seed_mcp(&state);
        let mut calls = 0;
        let error = with_hook(
            Box::new(move |resource, replacement| {
                calls += 1;
                if calls == 4 {
                    resource.write(replacement.unwrap())?;
                    return Err(AppError::io(
                        resource.path(),
                        std::io::Error::other("fixture final MCP failure"),
                    ));
                }
                Ok(())
            }),
            || ProviderService::switch(&state, AppType::Gemini, "new"),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("fixture final MCP failure"),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(get_gemini_settings_path()).unwrap(),
            large
        );
        assert_eq!(
            fs::read(get_gemini_env_path()).ok().as_deref(),
            (!missing_env).then_some(OLD_ENV.as_bytes())
        );
        assert_old_selection(&state);
    }
}

#[test]
fn gemini_switch_success_keeps_native_fields_and_all_mcp_followups() {
    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    seed_native();
    let state = state();
    seed_mcp(&state);
    ProviderService::switch(&state, AppType::Gemini, "new").unwrap();
    let settings = read_json_file::<Value>(&get_gemini_settings_path()).unwrap();
    assert_eq!(settings["advanced"], json!({"opaque":true}));
    assert_eq!(settings["theme"], "dark");
    for index in 0..2 {
        assert_eq!(
            settings["mcpServers"][format!("fixture-{index}")]["command"],
            "fixture-not-executed"
        );
    }
    assert_eq!(
        state.db.get_current_provider("gemini").unwrap().as_deref(),
        Some("new")
    );
    assert_eq!(
        crate::settings::get_current_provider(&AppType::Gemini).as_deref(),
        Some("new")
    );
    crate::mcp::remove_server_from_gemini("fixture-0").unwrap();
    assert!(!crate::gemini_mcp::read_mcp_servers_map()
        .unwrap()
        .contains_key("fixture-0"));
}

#[test]
fn gemini_switch_keeps_native_receipts_through_snapshot_persistence() {
    use crate::gemini_config::operation::with_hook;
    for after_native in [false, true] {
        let temp = TempDir::new().unwrap();
        let _guard = TestEnvGuard::isolated(temp.path());
        seed_native();
        let state = state();
        seed_mcp(&state);
        let condition = if after_native {
            "instr(NEW.settings_config, 'fixture-') > 0"
        } else {
            "NEW.is_current = 1"
        };
        for event in ["INSERT", "UPDATE"] {
            state
                .db
                .conn
                .lock()
                .unwrap()
                .execute_batch(&format!(
                    "CREATE TRIGGER fixture_{event} BEFORE {event} ON providers
                 WHEN NEW.id = 'new' AND NEW.app_type = 'gemini' AND {condition}
                 BEGIN SELECT RAISE(ABORT, 'fixture database failure'); END;"
                ))
                .unwrap();
        }
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = calls.clone();
        let error = with_hook(
            Box::new(move |_, _| {
                observed.set(observed.get() + 1);
                Ok(())
            }),
            || ProviderService::switch(&state, AppType::Gemini, "new"),
        )
        .unwrap_err();
        // Store intentionally hides raw SQLite diagnostics at the host boundary.
        assert!(matches!(error, AppError::Conflict(_)), "{error}");
        assert_eq!(calls.get(), if after_native { 6 } else { 0 });
        assert_restored_native_bytes();
        assert_old_selection(&state);
    }
}

#[test]
fn gemini_mcp_service_does_not_retain_state_guard_during_native_publication() {
    use crate::gemini_config::operation::with_hook;
    let temp = TempDir::new().unwrap();
    let _guard = TestEnvGuard::isolated(temp.path());
    seed_native();
    let state = std::sync::Arc::new(state());
    seed_mcp(&state);
    let observed = state.clone();
    let server = state.config.read().unwrap().mcp.servers.as_ref().unwrap()["fixture-0"].clone();
    with_hook(
        Box::new(move |_, _| {
            assert!(
                observed.config.try_write().is_ok(),
                "native publication must not retain a state read guard"
            );
            Ok(())
        }),
        || {
            McpService::upsert_server(&state, server).unwrap();
            McpService::toggle_app(&state, "fixture-0", AppType::Gemini, true).unwrap();
            McpService::sync_all_enabled(&state).unwrap();
        },
    );
}

#[cfg(unix)]
#[test]
fn gemini_switch_handles_indirect_link_observations_and_recovery() {
    use crate::gemini_config::operation::with_hook;
    use std::os::unix::fs::symlink;
    for layout in ["missing", "intermediate", "cycle"] {
        for (fail_at, publish_first) in [(0, false), (2, false), (2, true), (4, true)] {
            let temp = TempDir::new().unwrap();
            let _guard = TestEnvGuard::isolated(temp.path());
            seed_native();
            let env = get_gemini_env_path();
            let settings = get_gemini_settings_path();
            let relay = env.with_extension("relay");
            let external = temp.path().join("external.json");
            fs::write(&external, OLD_SETTINGS).unwrap();
            fs::remove_file(&env).unwrap();
            fs::remove_file(&settings).unwrap();
            match layout {
                "missing" => {
                    symlink(&env, &relay).unwrap();
                    symlink(&relay, &settings).unwrap();
                }
                "intermediate" => {
                    symlink(&external, &env).unwrap();
                    symlink(&env, &relay).unwrap();
                    symlink(&relay, &settings).unwrap();
                }
                "cycle" => {
                    symlink(&relay, &env).unwrap();
                    symlink(&settings, &relay).unwrap();
                    symlink(&env, &settings).unwrap();
                }
                _ => unreachable!(),
            }
            let original = (layout == "intermediate").then_some(OLD_SETTINGS.as_bytes());
            let state = state();
            seed_mcp(&state);
            let mut calls = 0;
            let result = with_hook(
                Box::new(move |resource, replacement| {
                    calls += 1;
                    if calls == fail_at {
                        if publish_first {
                            resource.write(replacement.unwrap())?;
                        }
                        return Err(AppError::io(
                            resource.path(),
                            std::io::Error::other("fixture indirect publication failure"),
                        ));
                    }
                    Ok(())
                }),
                || ProviderService::switch(&state, AppType::Gemini, "new"),
            );
            assert_eq!(fs::read_to_string(external).unwrap(), OLD_SETTINGS);
            if fail_at == 0 {
                result.unwrap();
                assert_eq!(fs::read_to_string(env).unwrap(), "GEMINI_API_KEY=new-fake");
                assert_eq!(read_json_file::<Value>(&settings).unwrap()["theme"], "dark");
            } else {
                let error = result.unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("fixture indirect publication failure"),
                    "{layout}: {error}"
                );
                for path in [env, settings] {
                    assert_eq!(
                        fs::read(path).ok().as_deref(),
                        original,
                        "{layout}: {error}"
                    );
                }
                assert_old_selection(&state);
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn gemini_switch_keeps_external_changes_to_a_cross_target_link_referent() {
    use crate::gemini_config::operation::with_hook;
    use std::os::unix::fs::symlink;
    for (atomic_replace, retarget) in [(false, false), (true, false), (false, true)] {
        for external in ["{\"external\":true}", OLD_SETTINGS] {
            let temp = TempDir::new().unwrap();
            let _guard = TestEnvGuard::isolated(temp.path());
            seed_native();
            let source = get_gemini_env_path();
            let link = get_gemini_settings_path();
            let relay = source.with_extension("relay");
            fs::write(&source, OLD_SETTINGS).unwrap();
            fs::remove_file(&link).unwrap();
            if retarget {
                symlink(&source, &relay).unwrap();
            }
            symlink(if retarget { &relay } else { &source }, &link).unwrap();
            let state = state();
            seed_mcp(&state);
            let mut calls = 0;
            let error = with_hook(
                Box::new(move |resource, _| {
                    calls += 1;
                    if calls == 2 {
                        if atomic_replace || retarget {
                            let next = source.with_extension("external");
                            fs::write(&next, external).unwrap();
                            if retarget {
                                fs::remove_file(&relay).unwrap();
                                symlink(next, &relay).unwrap();
                            } else {
                                fs::rename(next, &source).unwrap();
                            }
                        } else {
                            fs::write(resource.path(), external).unwrap();
                        }
                    }
                    Ok(())
                }),
                || ProviderService::switch(&state, AppType::Gemini, "new"),
            )
            .unwrap_err();
            assert!(error.to_string().contains("Gemini"), "{error}");
            assert!(fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(fs::read_to_string(link).unwrap(), external);
            assert_eq!(
                fs::read_to_string(get_gemini_env_path()).unwrap(),
                if retarget {
                    "GEMINI_API_KEY=new-fake"
                } else {
                    external
                }
            );
            assert_old_selection(&state);
            assert_eq!(selected_type().as_deref(), Some("before"));
        }
    }
}

#[cfg(unix)]
#[test]
fn gemini_switch_handles_cross_target_links_at_each_publication_boundary() {
    use crate::gemini_config::operation::with_hook;
    use std::os::unix::fs::{symlink, PermissionsExt};
    for settings_points_to_env in [false, true] {
        for (fail_at, publish_first) in [
            (0, false),
            (1, false),
            (1, true),
            (2, false),
            (2, true),
            (4, false),
            (4, true),
        ] {
            let temp = TempDir::new().unwrap();
            let _guard = TestEnvGuard::isolated(temp.path());
            seed_native();
            // A JSON object is valid native settings and tolerated by the legacy env parser.
            let (source, link) = if settings_points_to_env {
                (get_gemini_env_path(), get_gemini_settings_path())
            } else {
                (get_gemini_settings_path(), get_gemini_env_path())
            };
            fs::write(&source, OLD_SETTINGS).unwrap();
            fs::remove_file(&link).unwrap();
            symlink(&source, &link).unwrap();
            let state = state();
            seed_mcp(&state);
            let mut calls = 0;
            let result = with_hook(
                Box::new(move |resource, replacement| {
                    calls += 1;
                    if calls == fail_at {
                        if publish_first {
                            resource.write(replacement.unwrap())?;
                        }
                        return Err(AppError::io(
                            resource.path(),
                            std::io::Error::other("fixture linked publication failure"),
                        ));
                    }
                    Ok(())
                }),
                || ProviderService::switch(&state, AppType::Gemini, "new"),
            );
            if fail_at != 0 {
                let error = result.unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("fixture linked publication failure"),
                    "{error}"
                );
                for path in [get_gemini_env_path(), get_gemini_settings_path()] {
                    assert_eq!(fs::read_to_string(path).unwrap(), OLD_SETTINGS, "settings_points_to_env={settings_points_to_env}, fail_at={fail_at}, publish_first={publish_first}");
                }
                assert_old_selection(&state);
                continue;
            }
            result.unwrap();
            assert!(!fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(
                fs::read_to_string(get_gemini_env_path()).unwrap(),
                "GEMINI_API_KEY=new-fake"
            );
            assert_eq!(
                read_json_file::<Value>(&get_gemini_settings_path()).unwrap()["theme"],
                "dark"
            );
            assert_eq!(
                fs::metadata(get_gemini_env_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
