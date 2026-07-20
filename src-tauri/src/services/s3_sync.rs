//! S3-compatible implementation of the shared v2 cloud-sync protocol.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::OnceLock;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::settings::{
    get_s3_sync_settings, update_s3_sync_status, S3SyncSettings, WebDavSyncStatus,
};

use super::s3::{self, S3Credentials};
use super::sync_protocol::{
    apply_snapshot_with_restore_guard, build_local_snapshot, localized, sha256_hex,
    validate_artifact_size_limit, validate_manifest_compat, verify_artifact, ArtifactMeta,
    RemoteLayout, SyncManifest, DB_COMPAT_VERSION, MAX_MANIFEST_BYTES, MAX_SYNC_ARTIFACT_BYTES,
    PROTOCOL_VERSION, REMOTE_DB_SQL, REMOTE_MANIFEST, REMOTE_SKILLS_ZIP,
};
use super::webdav_sync::SyncDecision;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3SyncSummary {
    pub decision: SyncDecision,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct S3RemoteInfo {
    pub device_name: String,
    pub created_at: String,
    pub snapshot_id: String,
    pub version: u32,
    pub protocol_version: u32,
    pub db_compat_version: Option<u32>,
    pub compatible: bool,
    pub artifacts: Vec<String>,
    pub layout: String,
    pub remote_path: String,
}

pub struct S3SyncService;

fn sync_mutex() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn run_with_sync_lock<T, Fut>(operation: Fut) -> Result<T, AppError>
where
    Fut: Future<Output = Result<T, AppError>>,
{
    let _guard = sync_mutex().lock().await;
    operation.await
}

impl S3SyncService {
    /// Check saved credentials. Unlike transfer operations, this intentionally
    /// permits a configured-but-disabled backend so users can test before
    /// enabling it.
    pub fn check_connection() -> Result<(), AppError> {
        run_http(async {
            let settings = load_s3_settings(false)?;
            settings.validate()?;
            s3::test_connection(&credentials_for(&settings)).await
        })
    }

    pub fn upload() -> Result<S3SyncSummary, AppError> {
        let result = run_http(run_with_sync_lock(upload()));
        if let Err(error) = &result {
            persist_sync_error_best_effort(error, "manual");
        }
        result
    }

    pub fn download() -> Result<S3SyncSummary, AppError> {
        let result = run_http(run_with_sync_lock(download()));
        if let Err(error) = &result {
            persist_sync_error_best_effort(error, "manual");
        }
        result
    }

    pub fn fetch_remote_info() -> Result<Option<S3RemoteInfo>, AppError> {
        run_http(fetch_remote_info())
    }
}

async fn upload() -> Result<S3SyncSummary, AppError> {
    let mut settings = load_s3_settings(true)?;
    settings.validate()?;
    let credentials = credentials_for(&settings);
    let snapshot = build_local_snapshot()?;

    let database_key = s3_key(&settings, REMOTE_DB_SQL);
    s3::put_object(
        &credentials,
        &database_key,
        snapshot.db_sql,
        "application/sql",
    )
    .await?;

    let skills_key = s3_key(&settings, REMOTE_SKILLS_ZIP);
    s3::put_object(
        &credentials,
        &skills_key,
        snapshot.skills_zip,
        "application/zip",
    )
    .await?;

    // The manifest is the commit marker and must be uploaded last.
    let manifest_key = s3_key(&settings, REMOTE_MANIFEST);
    s3::put_object(
        &credentials,
        &manifest_key,
        snapshot.manifest_bytes,
        "application/json",
    )
    .await?;

    let etag = match s3::head_object(&credentials, &manifest_key).await {
        Ok(etag) => etag,
        Err(error) => {
            log::debug!("[S3] Failed to fetch ETag after upload: {error}");
            None
        }
    };
    persist_sync_success_best_effort(&mut settings, &snapshot.manifest_hash, etag);

    Ok(S3SyncSummary {
        decision: SyncDecision::Upload,
        message: "S3 upload completed".to_string(),
    })
}

async fn download() -> Result<S3SyncSummary, AppError> {
    let mut settings = load_s3_settings(true)?;
    settings.validate()?;
    let credentials = credentials_for(&settings);
    let manifest_key = s3_key(&settings, REMOTE_MANIFEST);
    let (manifest_bytes, etag) = s3::get_object(&credentials, &manifest_key, MAX_MANIFEST_BYTES)
        .await?
        .ok_or_else(|| {
            localized(
                "s3.sync.remote_empty",
                "远端没有可下载的同步数据",
                "No downloadable sync data found on the remote.",
            )
        })?;
    let manifest: SyncManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|source| AppError::Json {
            path: REMOTE_MANIFEST.to_string(),
            source,
        })?;
    validate_manifest_compat(&manifest, RemoteLayout::Current)?;

    let database =
        download_and_verify(&settings, &credentials, REMOTE_DB_SQL, &manifest.artifacts).await?;
    let skills = download_and_verify(
        &settings,
        &credentials,
        REMOTE_SKILLS_ZIP,
        &manifest.artifacts,
    )
    .await?;

    apply_snapshot_with_restore_guard(&database, &skills).await?;

    let manifest_hash = sha256_hex(&manifest_bytes);
    persist_sync_success_best_effort(&mut settings, &manifest_hash, etag);
    Ok(S3SyncSummary {
        decision: SyncDecision::Download,
        message: "S3 download completed".to_string(),
    })
}

async fn fetch_remote_info() -> Result<Option<S3RemoteInfo>, AppError> {
    let settings = load_s3_settings(true)?;
    settings.validate()?;
    let credentials = credentials_for(&settings);
    let manifest_key = s3_key(&settings, REMOTE_MANIFEST);
    let Some((bytes, _)) = s3::get_object(&credentials, &manifest_key, MAX_MANIFEST_BYTES).await?
    else {
        return Ok(None);
    };
    let manifest: SyncManifest =
        serde_json::from_slice(&bytes).map_err(|source| AppError::Json {
            path: REMOTE_MANIFEST.to_string(),
            source,
        })?;
    let compatible = validate_manifest_compat(&manifest, RemoteLayout::Current).is_ok();
    let artifacts = manifest.artifacts.keys().cloned().collect();

    Ok(Some(S3RemoteInfo {
        device_name: manifest.device_name,
        created_at: manifest.created_at,
        snapshot_id: manifest.snapshot_id,
        version: manifest.version,
        protocol_version: manifest.version,
        db_compat_version: manifest.db_compat_version,
        compatible,
        artifacts,
        layout: RemoteLayout::Current.as_str().to_string(),
        remote_path: s3_directory_display(&settings),
    }))
}

async fn download_and_verify(
    settings: &S3SyncSettings,
    credentials: &S3Credentials,
    artifact_name: &str,
    artifacts: &BTreeMap<String, ArtifactMeta>,
) -> Result<Vec<u8>, AppError> {
    let metadata = artifacts.get(artifact_name).ok_or_else(|| {
        localized(
            "s3.sync.manifest_missing_artifact",
            format!("manifest 中缺少 artifact: {artifact_name}"),
            format!("Manifest missing artifact: {artifact_name}"),
        )
    })?;
    validate_artifact_size_limit(artifact_name, metadata.size)?;

    let key = s3_key(settings, artifact_name);
    let (bytes, _) = s3::get_object(credentials, &key, MAX_SYNC_ARTIFACT_BYTES as usize)
        .await?
        .ok_or_else(|| {
            localized(
                "s3.sync.remote_missing_artifact",
                format!("远端缺少 artifact 文件: {artifact_name}"),
                format!("Remote artifact file missing: {artifact_name}"),
            )
        })?;
    verify_artifact(&bytes, artifact_name, metadata)?;
    Ok(bytes)
}

fn load_s3_settings(require_enabled: bool) -> Result<S3SyncSettings, AppError> {
    let settings = get_s3_sync_settings().ok_or_else(|| {
        localized(
            "s3.sync.not_configured",
            "未配置 S3 同步",
            "S3 sync is not configured.",
        )
    })?;
    if require_enabled && !settings.enabled {
        return Err(localized(
            "s3.sync.not_enabled",
            "S3 同步未启用",
            "S3 sync is not enabled.",
        ));
    }
    Ok(settings)
}

fn persist_sync_success(
    settings: &mut S3SyncSettings,
    manifest_hash: &str,
    etag: Option<String>,
) -> Result<(), AppError> {
    let status = WebDavSyncStatus {
        last_sync_at: Some(Utc::now().timestamp()),
        last_error: None,
        last_error_source: None,
        last_remote_etag: etag,
        last_local_manifest_hash: Some(manifest_hash.to_string()),
        last_remote_manifest_hash: Some(manifest_hash.to_string()),
    };
    settings.status = status.clone();
    update_s3_sync_status(status)
}

fn persist_sync_success_best_effort(
    settings: &mut S3SyncSettings,
    manifest_hash: &str,
    etag: Option<String>,
) {
    if let Err(error) = persist_sync_success(settings, manifest_hash, etag) {
        log::warn!("[S3] Failed to persist sync status (non-fatal): {error}");
    }
}

fn persist_sync_error_best_effort(error: &AppError, source: &str) {
    let Some(mut settings) = get_s3_sync_settings() else {
        return;
    };
    settings.status.last_error = Some(error.to_string());
    settings.status.last_error_source = Some(source.to_string());
    if let Err(persist_error) = update_s3_sync_status(settings.status) {
        log::warn!("[S3] Failed to persist sync error (non-fatal): {persist_error}");
    }
}

fn credentials_for(settings: &S3SyncSettings) -> S3Credentials {
    S3Credentials {
        access_key_id: settings.access_key_id.clone(),
        secret_access_key: settings.secret_access_key.clone(),
        region: settings.region.clone(),
        bucket: settings.bucket.clone(),
        endpoint: settings.endpoint.clone(),
    }
}

fn s3_key(settings: &S3SyncSettings, artifact: &str) -> String {
    format!(
        "{}/v{}/db-v{}/{}/{}",
        settings.remote_root, PROTOCOL_VERSION, DB_COMPAT_VERSION, settings.profile, artifact
    )
}

fn s3_directory_display(settings: &S3SyncSettings) -> String {
    format!(
        "{}/v{}/db-v{}/{}",
        settings.remote_root, PROTOCOL_VERSION, DB_COMPAT_VERSION, settings.profile
    )
}

fn run_http<F, T>(future: F) -> Result<T, AppError>
where
    F: std::future::Future<Output = Result<T, AppError>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            localized(
                "s3.sync.runtime_create_failed",
                format!("创建异步运行时失败: {error}"),
                format!("Failed to create async runtime: {error}"),
            )
        })?;
    runtime.block_on(future)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_matches_upstream_v2_db_v6_layout() {
        let settings = S3SyncSettings {
            remote_root: "cc-switch-sync".to_string(),
            profile: "default".to_string(),
            ..S3SyncSettings::default()
        };
        assert_eq!(
            s3_key(&settings, REMOTE_MANIFEST),
            "cc-switch-sync/v2/db-v6/default/manifest.json"
        );
    }

    #[test]
    fn remote_info_serialization_matches_upstream_field_names() {
        let info = S3RemoteInfo {
            device_name: "dev".to_string(),
            created_at: "now".to_string(),
            snapshot_id: "snapshot".to_string(),
            version: 2,
            protocol_version: 2,
            db_compat_version: Some(6),
            compatible: true,
            artifacts: vec!["db.sql".to_string()],
            layout: "current".to_string(),
            remote_path: "root/v2/db-v6/default".to_string(),
        };
        let value = serde_json::to_value(info).expect("serialize remote info");
        assert_eq!(value["deviceName"], "dev");
        assert_eq!(value["protocolVersion"], 2);
        assert_eq!(value["dbCompatVersion"], 6);
        assert_eq!(value["remotePath"], "root/v2/db-v6/default");
    }
}
