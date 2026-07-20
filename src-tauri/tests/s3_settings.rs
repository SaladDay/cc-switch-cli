use cc_switch_lib::{
    check_permissions, get_app_config_dir, get_s3_sync_settings, get_webdav_sync_settings,
    set_s3_sync_settings, set_webdav_sync_settings, S3SyncSettings, WebDavSyncSettings,
    WebDavSyncStatus,
};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex, reset_test_fs};

fn sample_s3(enabled: bool) -> S3SyncSettings {
    S3SyncSettings {
        enabled,
        auto_sync: false,
        region: " us-east-1 ".to_string(),
        bucket: " example-bucket ".to_string(),
        access_key_id: " AKID ".to_string(),
        secret_access_key: "plain-secret".to_string(),
        endpoint: " https://s3.example.com/ ".to_string(),
        remote_root: " cc-switch-sync ".to_string(),
        profile: " default ".to_string(),
        status: WebDavSyncStatus::default(),
    }
}

fn sample_webdav(enabled: bool) -> WebDavSyncSettings {
    WebDavSyncSettings {
        enabled,
        base_url: "https://dav.example.com/root".to_string(),
        remote_root: "cc-switch-sync".to_string(),
        profile: "default".to_string(),
        username: "demo".to_string(),
        password: "webdav-secret".to_string(),
        auto_sync: true,
        status: WebDavSyncStatus {
            last_remote_etag: Some("webdav-etag".to_string()),
            ..WebDavSyncStatus::default()
        },
    }
}

#[test]
fn s3_settings_persist_with_upstream_field_names_and_normalized_values() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    set_s3_sync_settings(Some(sample_s3(false))).expect("save S3 settings");
    let saved = get_s3_sync_settings().expect("S3 settings should be present");
    assert_eq!(saved.region, "us-east-1");
    assert_eq!(saved.bucket, "example-bucket");
    assert_eq!(saved.access_key_id, "AKID");
    assert_eq!(saved.secret_access_key, "plain-secret");
    assert_eq!(saved.endpoint, "https://s3.example.com/");
    assert_eq!(saved.remote_root, "cc-switch-sync");
    assert_eq!(saved.profile, "default");

    let json = std::fs::read_to_string(get_app_config_dir().join("settings.json"))
        .expect("read settings.json");
    assert!(
        json.contains("\"s3Sync\""),
        "unexpected settings JSON: {json}"
    );
    assert!(
        json.contains("\"secretAccessKey\": \"plain-secret\""),
        "S3 secret should use the upstream camelCase key"
    );
}

#[test]
fn s3_settings_reject_missing_required_credentials() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    let mut settings = sample_s3(false);
    settings.bucket.clear();
    let error = set_s3_sync_settings(Some(settings)).expect_err("empty bucket must be rejected");
    assert!(
        error.to_string().to_lowercase().contains("bucket") || error.to_string().contains("存储桶"),
        "unexpected error: {error}"
    );
}

#[test]
fn webdav_and_s3_settings_can_be_enabled_together_like_upstream() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    set_webdav_sync_settings(Some(sample_webdav(true))).expect("save WebDAV settings");

    let mut s3 = sample_s3(true);
    s3.auto_sync = true;
    s3.status.last_remote_etag = Some("s3-etag".to_string());
    set_s3_sync_settings(Some(s3)).expect("enable S3");

    let webdav = get_webdav_sync_settings().expect("WebDAV settings should be retained");
    assert!(webdav.enabled);
    assert!(webdav.auto_sync);
    assert_eq!(webdav.password, "webdav-secret");
    assert_eq!(
        webdav.status.last_remote_etag.as_deref(),
        Some("webdav-etag")
    );

    let s3 = get_s3_sync_settings().expect("S3 settings should be retained");
    assert!(s3.enabled);
    assert!(s3.auto_sync);
    assert_eq!(s3.secret_access_key, "plain-secret");
    assert_eq!(s3.status.last_remote_etag.as_deref(), Some("s3-etag"));
}

#[cfg(unix)]
#[test]
fn s3_credentials_are_written_to_owner_only_settings_file() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    set_s3_sync_settings(Some(sample_s3(false))).expect("save S3 settings");
    let path = get_app_config_dir().join("settings.json");
    let mode = std::fs::metadata(&path)
        .expect("settings.json metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    assert!(check_permissions().is_empty());
}

#[test]
fn s3_settings_can_be_cleared_without_touching_webdav() {
    let _guard = lock_test_mutex();
    reset_test_fs();
    let _home = ensure_test_home();

    set_webdav_sync_settings(Some(sample_webdav(false))).expect("save WebDAV settings");
    set_s3_sync_settings(Some(sample_s3(false))).expect("save S3 settings");
    set_s3_sync_settings(None).expect("clear S3 settings");

    assert!(get_s3_sync_settings().is_none());
    assert!(get_webdav_sync_settings().is_some());
}
