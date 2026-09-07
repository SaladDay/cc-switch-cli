use super::*;
use crate::{gemini_mcp::*, test_support::TestEnvGuard, MultiAppConfig};
use serde_json::{json, Value};
use std::collections::HashMap;

const ORIGINAL: &[u8] =
    b"{ \"opaque\": {\"keep\":true}, \"mcpServers\": {\"old\":{\"command\":\"node\"}} }\n";
const EXTERNAL: &[u8] = b"{\"external\":true,\"mcpServers\":{\"other\":{\"command\":\"user\"}}}";

fn seed(contents: Option<&[u8]>) -> PathBuf {
    let path = user_config_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    if let Some(contents) = contents {
        fs::write(&path, contents).unwrap();
    } else if path.exists() {
        fs::remove_file(&path).unwrap();
    }
    path
}

fn with_hook<T>(hook: ExchangeHook, action: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            BEFORE_EXCHANGE.with(|slot| *slot.borrow_mut() = None);
        }
    }
    BEFORE_EXCHANGE.with(|slot| *slot.borrow_mut() = Some(hook));
    let _reset = Reset;
    action()
}

fn injected_error(path: &Path, message: &'static str) -> AppError {
    AppError::io(path, std::io::Error::other(message))
}

#[test]
fn mcp_mutation_keeps_its_first_observation_and_preserves_external_changes() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    for original in [None, Some(ORIGINAL)] {
        let path = seed(original);
        let error = update_mcp_servers_map(|servers| {
            fs::write(&path, EXTERNAL).unwrap();
            servers.insert("added".into(), json!({"command":"new"}));
        })
        .unwrap_err();
        assert!(matches!(error, AppError::Conflict(_)), "{error}");
        assert_eq!(fs::read(&path).unwrap(), EXTERNAL);
    }
    let path = seed(Some(ORIGINAL));
    let error = update_mcp_servers_map(|servers| {
        fs::remove_file(&path).unwrap();
        servers.remove("old");
    })
    .unwrap_err();
    assert!(matches!(error, AppError::Conflict(_)), "{error}");
    assert!(!path.exists());
}

#[test]
fn mcp_callers_preserve_native_fields_and_check_again_at_exchange() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let path = seed(Some(ORIGINAL));
    let config = MultiAppConfig::default();
    crate::mcp::sync_single_server_to_gemini(
        &config,
        "new",
        &json!({"type":"http","url":"https://example.com/mcp","timeout":123}),
    )
    .unwrap();
    let root: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(root["opaque"], json!({"keep":true}));
    assert_eq!(root["mcpServers"]["old"]["command"], "node");
    assert_eq!(
        root["mcpServers"]["new"],
        json!({"httpUrl":"https://example.com/mcp","timeout":60000})
    );
    crate::mcp::remove_server_from_gemini("old").unwrap();
    assert!(!read_mcp_servers_map().unwrap().contains_key("old"));
    let error = with_hook(
        Box::new(|resource, _| {
            fs::write(resource.path(), EXTERNAL).unwrap();
            Ok(())
        }),
        || crate::mcp::remove_server_from_gemini("new"),
    )
    .unwrap_err();
    assert!(matches!(error, AppError::Conflict(_)), "{error}");
    assert_eq!(fs::read(&path).unwrap(), EXTERNAL);
}

#[test]
fn mcp_write_failure_recovers_original_bytes_or_missing_file() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    for original in [None, Some(ORIGINAL)] {
        for publish_first in [false, true] {
            let path = seed(original);
            let mut first = true;
            let error = with_hook(
                Box::new(move |resource, replacement| {
                    if !std::mem::take(&mut first) {
                        return Ok(());
                    }
                    if publish_first {
                        resource.write(replacement.unwrap())?;
                    }
                    Err(injected_error(resource.path(), "publish failure"))
                }),
                || set_mcp_servers_map(&HashMap::new()),
            )
            .unwrap_err();
            assert!(matches!(error, AppError::Io { .. }), "{error}");
            assert_eq!(fs::read(&path).ok().as_deref(), original);
        }
    }
}

#[test]
fn mcp_incomplete_recovery_reports_both_errors_and_keeps_external_contents() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    for external_edit in [false, true] {
        let path = seed(Some(ORIGINAL));
        let mut first = true;
        let error = with_hook(
            Box::new(move |resource, replacement| {
                if std::mem::take(&mut first) {
                    resource.write(replacement.unwrap())?;
                    return Err(injected_error(resource.path(), "publish failure"));
                }
                if external_edit {
                    fs::write(resource.path(), EXTERNAL).unwrap();
                    Ok(())
                } else {
                    Err(injected_error(resource.path(), "recovery failure"))
                }
            }),
            || set_mcp_servers_map(&HashMap::new()),
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                AppError::Localized {
                    key: "gemini.mcp.rollback_failed",
                    ..
                }
            ),
            "{error}"
        );
        assert!(error.to_string().contains("publish failure"));
        if external_edit {
            assert_eq!(fs::read(&path).unwrap(), EXTERNAL);
        } else {
            assert!(error.to_string().contains("recovery failure"));
            let root: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(root["mcpServers"], json!({}));
        }
    }
}

#[test]
fn successful_mcp_receipt_holds_the_lock_and_recovers_large_original_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let large = format!(
        "{{ \"opaque\":\"{}\" }}\n",
        "x".repeat(cc_switch_core::MAX_OPERATION_CONTENT_BYTES + 1)
    );
    for external_edit in [false, true] {
        let path = seed(Some(large.as_bytes()));
        let operation = SettingsOperation::observe(&path).unwrap();
        let root = parse_json_value(&path, operation.contents()).unwrap();
        let mut published = publish_servers(operation, root, &HashMap::new()).unwrap();
        assert!(matches!(
            SETTINGS_WRITE_LOCK.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        if external_edit {
            fs::write(&path, EXTERNAL).unwrap();
        }
        let result = published.receipt.rollback(&mut published.host);
        if external_edit {
            assert!(result.is_err());
            assert_eq!(fs::read(&path).unwrap(), EXTERNAL);
        } else {
            result.unwrap();
            assert_eq!(fs::read(&path).unwrap(), large.as_bytes());
        }
        drop(published.host);
        assert!(SETTINGS_WRITE_LOCK.try_lock().is_ok());
    }
}

#[test]
fn mcp_read_and_validation_do_not_create_directories() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let path = user_config_path();
    assert!(read_mcp_servers_map().unwrap().is_empty());
    assert_eq!(read_mcp_json().unwrap(), None);
    assert!(set_mcp_servers_map(&HashMap::from([("invalid".into(), Value::Null)])).is_err());
    assert!(!path.parent().unwrap().exists());
    set_mcp_servers_map(&HashMap::new()).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "{\n  \"mcpServers\": {}\n}"
    );
}

#[cfg(unix)]
#[test]
fn mcp_paths_reject_parent_retargeting_and_preserve_leaf_policy() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let path = user_config_path();
    let original_dir = temp.path().join("original");
    let other_dir = temp.path().join("other");
    fs::create_dir(&original_dir).unwrap();
    fs::create_dir(&other_dir).unwrap();
    fs::write(original_dir.join("settings.json"), ORIGINAL).unwrap();
    fs::write(other_dir.join("settings.json"), ORIGINAL).unwrap();
    symlink(&original_dir, path.parent().unwrap()).unwrap();
    let operation = SettingsOperation::observe(&path).unwrap();
    fs::remove_file(path.parent().unwrap()).unwrap();
    symlink(&other_dir, path.parent().unwrap()).unwrap();
    let error = operation.execute("{}".into()).err().unwrap();
    assert!(matches!(error, AppError::Conflict(_)), "{error}");
    assert_eq!(
        fs::read(original_dir.join("settings.json")).unwrap(),
        ORIGINAL
    );
    assert_eq!(fs::read(&path).unwrap(), ORIGINAL);

    let missing_path = path.parent().unwrap().join("nested/deeper/settings.json");
    let operation = SettingsOperation::observe(&missing_path).unwrap();
    fs::remove_file(path.parent().unwrap()).unwrap();
    symlink(&original_dir, path.parent().unwrap()).unwrap();
    assert!(matches!(
        operation.execute("{}".into()),
        Err(AppError::Conflict(_))
    ));
    assert!(!original_dir.join("nested").exists());
    assert!(!other_dir.join("nested").exists());

    fs::remove_file(&path).unwrap();
    let source = temp.path().join("source.json");
    fs::write(&source, ORIGINAL).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&source, &path).unwrap();
    set_mcp_servers_map(&HashMap::new()).unwrap();
    assert!(!fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(&source).unwrap(), ORIGINAL);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[cfg(unix)]
#[test]
fn mcp_custom_managed_paths_keep_existing_permission_policy_and_read_only_imports() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    for (suffix, expected_mode) in [("gemini/nested", 0o640), ("backups/gemini", 0o600)] {
        let custom = crate::config::get_app_config_dir().join(suffix);
        let mut settings = crate::settings::get_settings();
        settings.gemini_config_dir = Some(custom.to_string_lossy().into_owned());
        crate::settings::update_settings(settings).unwrap();
        let path = seed(Some(ORIGINAL));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(read_mcp_servers_map().unwrap().contains_key("old"));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        set_mcp_servers_map(&HashMap::new()).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            expected_mode
        );
        assert_eq!(path, custom.join("settings.json"));
    }
}

#[cfg(unix)]
#[test]
fn mcp_custom_paths_keep_relative_and_missing_parent_component_support() {
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let mut relative = PathBuf::new();
    for component in std::env::current_dir().unwrap().components() {
        if matches!(component, Component::Normal(_)) {
            relative.push("..");
        }
    }
    relative.push(temp.path().strip_prefix("/").unwrap());
    for (base, label) in [
        (relative, "relative"),
        (temp.path().to_path_buf(), "absolute"),
    ] {
        let custom = base.join(label).join("missing/../gemini");
        let mut settings = crate::settings::get_settings();
        settings.gemini_config_dir = Some(custom.to_string_lossy().into_owned());
        crate::settings::update_settings(settings).unwrap();
        assert!(read_mcp_servers_map().unwrap().is_empty());
        assert!(!temp.path().join(label).exists());
        set_mcp_servers_map(&HashMap::new()).unwrap();
        let output = temp.path().join(label).join("gemini/settings.json");
        assert_eq!(
            fs::read_to_string(output).unwrap(),
            "{\n  \"mcpServers\": {}\n}"
        );
    }
}

#[cfg(windows)]
#[test]
fn mcp_windows_failed_replacement_keeps_the_previous_file() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let _env = TestEnvGuard::isolated(temp.path());
    let path = seed(Some(ORIGINAL));
    let mut first = true;
    let error = with_hook(
        Box::new(move |resource, replacement| {
            if !std::mem::take(&mut first) {
                return Ok(());
            }
            let held = fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(resource.path())
                .unwrap();
            let result = resource.write(replacement.unwrap());
            drop(held);
            assert_eq!(fs::read(resource.path()).unwrap(), ORIGINAL);
            assert!(result.is_err());
            result
        }),
        || set_mcp_servers_map(&HashMap::new()),
    )
    .unwrap_err();
    assert!(
        matches!(error, AppError::Io { .. } | AppError::IoContext { .. }),
        "{error}"
    );
    assert_eq!(fs::read(&path).unwrap(), ORIGINAL);
}
