//! WebDAV v2 sync protocol layer with DB compatibility subdirectories.
//!
//! Manifest-based synchronization on top of the WebDAV transport helpers.
//! Current layout uses `{root}/v2/db-v6/{profile}/`, with legacy fallback to
//! `{root}/v2/{profile}/`. Artifact set: `db.sql` + `skills.zip`.

pub(crate) mod archive;

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::OnceLock;

use chrono::Utc;
use serde::Deserialize;

use crate::error::AppError;
use crate::services::webdav;
use crate::settings::{
    get_webdav_sync_settings, update_webdav_sync_status, WebDavSyncSettings, WebDavSyncStatus,
};

use super::sync_protocol::{
    apply_snapshot_with_restore_guard, build_local_snapshot, localized, sha256_hex,
    validate_artifact_size_limit, validate_manifest_compat, verify_artifact, ArtifactMeta,
    RemoteLayout, SyncManifest, DB_COMPAT_VERSION, MAX_MANIFEST_BYTES, MAX_SYNC_ARTIFACT_BYTES,
    PROTOCOL_FORMAT, PROTOCOL_VERSION, REMOTE_DB_SQL, REMOTE_MANIFEST, REMOTE_SKILLS_ZIP,
};

#[cfg(test)]
use super::sync_protocol::{
    apply_snapshot, compute_snapshot_id, detect_system_device_name, effective_db_compat_version,
    extract_sql_user_version, normalize_device_name, validate_sql_user_version_for_import,
    LEGACY_DB_COMPAT_VERSION, MAX_DEVICE_NAME_LEN,
};

#[cfg(test)]
use crate::database::{Database, SCHEMA_VERSION};

// ---------------------------------------------------------------------------
// 公共类型
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDecision {
    Upload,
    Download,
    /// V2 远端为空，但检测到 V1 数据，需要用户确认迁移
    V1MigrationNeeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebDavSyncSummary {
    pub decision: SyncDecision,
    pub message: String,
}

struct RemoteSnapshot {
    layout: RemoteLayout,
    manifest: SyncManifest,
    manifest_bytes: Vec<u8>,
    manifest_etag: Option<String>,
}

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

// ---------------------------------------------------------------------------
// 公共 API（同步包装）
// ---------------------------------------------------------------------------

pub struct WebDavSyncService;

impl WebDavSyncService {
    pub fn check_connection() -> Result<(), AppError> {
        run_http(check_connection())
    }

    pub fn upload() -> Result<WebDavSyncSummary, AppError> {
        run_http(run_with_sync_lock(upload()))
    }

    pub fn download() -> Result<WebDavSyncSummary, AppError> {
        run_http(run_with_sync_lock(download()))
    }

    /// 用户确认后调用：下载 V1 数据 → 应用 → 上传 V2 → 删除 V1
    pub fn migrate_v1_to_v2() -> Result<WebDavSyncSummary, AppError> {
        run_http(run_with_sync_lock(migrate_v1_to_v2()))
    }
}

// ---------------------------------------------------------------------------
// 异步核心
// ---------------------------------------------------------------------------

async fn check_connection() -> Result<(), AppError> {
    // Connectivity checks are also used immediately after saving a disabled
    // backend. Saving credentials must not implicitly enable WebDAV.
    let settings = load_webdav_settings(false)?;
    let auth = webdav::auth_from_credentials(&settings.username, &settings.password);
    webdav::test_connection(&settings.base_url, &auth).await?;
    let dir_segments = remote_dir_segments(&settings, RemoteLayout::Current);
    webdav::ensure_remote_directories(&settings.base_url, &dir_segments, &auth).await?;
    Ok(())
}

async fn upload() -> Result<WebDavSyncSummary, AppError> {
    let mut settings = load_webdav_settings(true)?;
    let auth = webdav::auth_from_credentials(&settings.username, &settings.password);

    let dir_segments = remote_dir_segments(&settings, RemoteLayout::Current);
    webdav::ensure_remote_directories(&settings.base_url, &dir_segments, &auth).await?;

    let snapshot = build_local_snapshot()?;

    // 上传 artifacts
    let db_url = build_artifact_url(&settings, RemoteLayout::Current, REMOTE_DB_SQL)?;
    webdav::put_bytes(&db_url, &auth, snapshot.db_sql, "application/sql").await?;

    let skills_url = build_artifact_url(&settings, RemoteLayout::Current, REMOTE_SKILLS_ZIP)?;
    webdav::put_bytes(&skills_url, &auth, snapshot.skills_zip, "application/zip").await?;

    // 上传 manifest（最后上传，确保 artifacts 已就绪）
    let manifest_url = build_artifact_url(&settings, RemoteLayout::Current, REMOTE_MANIFEST)?;
    webdav::put_bytes(
        &manifest_url,
        &auth,
        snapshot.manifest_bytes,
        "application/json",
    )
    .await?;

    // 获取 etag（best-effort，不影响上传结果）
    let etag = match webdav::head_etag(&manifest_url, &auth).await {
        Ok(e) => e,
        Err(e) => {
            log::debug!("[WebDAV] Failed to fetch ETag after upload: {e}");
            None
        }
    };

    persist_sync_success_best_effort(&mut settings, &snapshot.manifest_hash, etag);

    Ok(WebDavSyncSummary {
        decision: SyncDecision::Upload,
        message: "WebDAV upload completed".to_string(),
    })
}

async fn download() -> Result<WebDavSyncSummary, AppError> {
    let mut settings = load_webdav_settings(true)?;
    let auth = webdav::auth_from_credentials(&settings.username, &settings.password);

    if let Some(snapshot) = find_remote_snapshot(&settings, &auth).await? {
        validate_manifest_compat(&snapshot.manifest, snapshot.layout)?;

        let manifest_hash = sha256_hex(&snapshot.manifest_bytes);
        let db_sql = download_and_verify(
            &settings,
            &auth,
            snapshot.layout,
            REMOTE_DB_SQL,
            &snapshot.manifest.artifacts,
        )
        .await?;
        let skills_zip = download_and_verify(
            &settings,
            &auth,
            snapshot.layout,
            REMOTE_SKILLS_ZIP,
            &snapshot.manifest.artifacts,
        )
        .await?;

        apply_snapshot_with_restore_guard(&db_sql, &skills_zip).await?;
        persist_sync_success_best_effort(&mut settings, &manifest_hash, snapshot.manifest_etag);
        cleanup_v1_remote(&settings, &auth).await;

        Ok(WebDavSyncSummary {
            decision: SyncDecision::Download,
            message: "WebDAV download completed".to_string(),
        })
    } else if detect_v1_manifest(&settings, &auth).await?.is_some() {
        Ok(WebDavSyncSummary {
            decision: SyncDecision::V1MigrationNeeded,
            message: String::new(),
        })
    } else {
        Err(localized(
            "webdav.sync.remote_empty",
            "远端没有可下载的同步数据",
            "No downloadable sync data found on the remote",
        ))
    }
}

// ---------------------------------------------------------------------------
// 设置加载 / 验证
// ---------------------------------------------------------------------------

fn load_webdav_settings(require_enabled: bool) -> Result<WebDavSyncSettings, AppError> {
    let settings = get_webdav_sync_settings().ok_or_else(|| {
        localized(
            "webdav.sync.not_configured",
            "未配置 WebDAV 同步",
            "WebDAV sync is not configured",
        )
    })?;
    if require_enabled && !settings.enabled {
        return Err(localized(
            "webdav.sync.not_enabled",
            "WebDAV 同步未启用",
            "WebDAV sync is not enabled",
        ));
    }
    settings.validate()?;
    Ok(settings)
}

// ---------------------------------------------------------------------------
// 远端路径
// ---------------------------------------------------------------------------

fn remote_dir_segments(settings: &WebDavSyncSettings, layout: RemoteLayout) -> Vec<String> {
    let mut segments = Vec::new();
    segments.extend(webdav::path_segments(&settings.remote_root).map(str::to_string));
    segments.push(format!("v{PROTOCOL_VERSION}"));
    if layout == RemoteLayout::Current {
        segments.push(format!("db-v{DB_COMPAT_VERSION}"));
    }
    segments.extend(webdav::path_segments(&settings.profile).map(str::to_string));
    segments
}

fn build_artifact_url(
    settings: &WebDavSyncSettings,
    layout: RemoteLayout,
    file_name: &str,
) -> Result<String, AppError> {
    let mut segments = remote_dir_segments(settings, layout);
    segments.extend(webdav::path_segments(file_name).map(str::to_string));
    webdav::build_remote_url(&settings.base_url, &segments)
}

async fn find_remote_snapshot(
    settings: &WebDavSyncSettings,
    auth: &webdav::WebDavAuth,
) -> Result<Option<RemoteSnapshot>, AppError> {
    if let Some(snapshot) = fetch_remote_snapshot(settings, auth, RemoteLayout::Current).await? {
        return Ok(Some(snapshot));
    }

    fetch_remote_snapshot(settings, auth, RemoteLayout::Legacy).await
}

async fn fetch_remote_snapshot(
    settings: &WebDavSyncSettings,
    auth: &webdav::WebDavAuth,
    layout: RemoteLayout,
) -> Result<Option<RemoteSnapshot>, AppError> {
    let manifest_url = build_artifact_url(settings, layout, REMOTE_MANIFEST)?;
    let Some((manifest_bytes, manifest_etag)) =
        webdav::get_bytes(&manifest_url, auth, Some(MAX_MANIFEST_BYTES as u64)).await?
    else {
        return Ok(None);
    };

    let manifest: SyncManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|e| AppError::Json {
            path: REMOTE_MANIFEST.to_string(),
            source: e,
        })?;

    Ok(Some(RemoteSnapshot {
        layout,
        manifest,
        manifest_bytes,
        manifest_etag,
    }))
}

// ---------------------------------------------------------------------------
// Artifact 下载 + 校验
// ---------------------------------------------------------------------------

async fn download_and_verify(
    settings: &WebDavSyncSettings,
    auth: &webdav::WebDavAuth,
    layout: RemoteLayout,
    artifact_name: &str,
    artifacts: &BTreeMap<String, ArtifactMeta>,
) -> Result<Vec<u8>, AppError> {
    let meta = artifacts.get(artifact_name).ok_or_else(|| {
        localized(
            "webdav.sync.manifest_missing_artifact",
            format!("manifest 中缺少 artifact: {artifact_name}"),
            format!("Manifest missing artifact: {artifact_name}"),
        )
    })?;

    validate_artifact_size_limit(artifact_name, meta.size)?;

    let url = build_artifact_url(settings, layout, artifact_name)?;
    let (bytes, _) = webdav::get_bytes(&url, auth, Some(MAX_SYNC_ARTIFACT_BYTES))
        .await?
        .ok_or_else(|| {
            localized(
                "webdav.sync.remote_missing_artifact",
                format!("远端缺少 artifact 文件: {artifact_name}"),
                format!("Remote artifact file missing: {artifact_name}"),
            )
        })?;

    verify_artifact(&bytes, artifact_name, meta)?;
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// 同步状态持久化
// ---------------------------------------------------------------------------

fn persist_sync_success(
    settings: &mut WebDavSyncSettings,
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
    update_webdav_sync_status(status)
}

/// 尽力持久化同步状态，失败时仅记录日志
fn persist_sync_success_best_effort(
    settings: &mut WebDavSyncSettings,
    manifest_hash: &str,
    etag: Option<String>,
) -> bool {
    match persist_sync_success(settings, manifest_hash, etag) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("持久化同步状态失败（非致命）: {e}");
            false
        }
    }
}

fn run_http<F, T>(future: F) -> Result<T, AppError>
where
    F: std::future::Future<Output = Result<T, AppError>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| {
            localized(
                "webdav.sync.runtime_create_failed",
                format!("创建异步运行时失败: {e}"),
                format!("Failed to create async runtime: {e}"),
            )
        })?;
    runtime.block_on(future)
}

// ---------------------------------------------------------------------------
// V1 → V2 迁移兼容
// ---------------------------------------------------------------------------

const V1_PROTOCOL_VERSION: u32 = 1;

/// V1 manifest 类型（仅用于反序列化旧数据）
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct V1Manifest {
    format: String,
    version: u32,
    #[allow(dead_code)]
    updated_at: String,
    #[allow(dead_code)]
    updated_by: String,
    artifacts: V1ManifestArtifacts,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct V1ManifestArtifacts {
    db_sql: V1ArtifactMeta,
    skills_zip: V1ArtifactMeta,
    #[allow(dead_code)]
    settings_sync: V1ArtifactMeta,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct V1ArtifactMeta {
    #[allow(dead_code)]
    path: String,
    sha256: String,
    size: u64,
}

fn v1_remote_dir_segments(settings: &WebDavSyncSettings) -> Vec<String> {
    let mut segments = Vec::new();
    segments.extend(webdav::path_segments(&settings.remote_root).map(str::to_string));
    segments.push(format!("v{V1_PROTOCOL_VERSION}"));
    segments.extend(webdav::path_segments(&settings.profile).map(str::to_string));
    segments
}

fn build_v1_artifact_url(
    settings: &WebDavSyncSettings,
    file_name: &str,
) -> Result<String, AppError> {
    let mut segments = v1_remote_dir_segments(settings);
    segments.extend(webdav::path_segments(file_name).map(str::to_string));
    webdav::build_remote_url(&settings.base_url, &segments)
}

/// 检测远端是否存在 V1 manifest，返回 Some(manifest) 或 None
async fn detect_v1_manifest(
    settings: &WebDavSyncSettings,
    auth: &webdav::WebDavAuth,
) -> Result<Option<V1Manifest>, AppError> {
    let url = build_v1_artifact_url(settings, REMOTE_MANIFEST)?;
    let result = webdav::get_bytes(&url, auth, Some(MAX_MANIFEST_BYTES as u64)).await?;
    match result {
        None => Ok(None),
        Some((bytes, _)) => {
            let manifest: V1Manifest = match serde_json::from_slice(&bytes) {
                Ok(m) => m,
                Err(e) => {
                    log::debug!("[WebDAV] V1 manifest parse failed, treating as absent: {e}");
                    return Ok(None);
                }
            };
            if manifest.format != PROTOCOL_FORMAT || manifest.version != V1_PROTOCOL_VERSION {
                return Ok(None);
            }
            Ok(Some(manifest))
        }
    }
}

/// 下载 V1 artifact 并校验
async fn download_v1_artifact(
    settings: &WebDavSyncSettings,
    auth: &webdav::WebDavAuth,
    file_name: &str,
    meta: &V1ArtifactMeta,
) -> Result<Vec<u8>, AppError> {
    if meta.size > MAX_SYNC_ARTIFACT_BYTES {
        let max_mb = MAX_SYNC_ARTIFACT_BYTES / 1024 / 1024;
        return Err(localized(
            "webdav.sync.v1_artifact_too_large",
            format!("V1 artifact {file_name} 超过下载上限（{max_mb} MB）"),
            format!("V1 artifact {file_name} exceeds download limit ({max_mb} MB)"),
        ));
    }

    let url = build_v1_artifact_url(settings, file_name)?;
    let (bytes, _) = webdav::get_bytes(&url, auth, Some(MAX_SYNC_ARTIFACT_BYTES))
        .await?
        .ok_or_else(|| {
            localized(
                "webdav.sync.v1_artifact_missing",
                format!("V1 远端缺少 artifact: {file_name}"),
                format!("V1 remote artifact missing: {file_name}"),
            )
        })?;

    if bytes.len() as u64 != meta.size {
        return Err(localized(
            "webdav.sync.v1_artifact_size_mismatch",
            format!("V1 artifact {file_name} 大小不匹配"),
            format!("V1 artifact {file_name} size mismatch"),
        ));
    }

    let actual_hash = sha256_hex(&bytes);
    if actual_hash != meta.sha256 {
        return Err(localized(
            "webdav.sync.v1_artifact_hash_mismatch",
            format!("V1 artifact {file_name} SHA256 校验失败"),
            format!("V1 artifact {file_name} SHA256 verification failed"),
        ));
    }

    Ok(bytes)
}

/// 删除 V1 远端目录（best-effort）
async fn cleanup_v1_remote(settings: &WebDavSyncSettings, auth: &webdav::WebDavAuth) {
    let segments = v1_remote_dir_segments(settings);
    let url = match webdav::build_remote_url(&settings.base_url, &segments) {
        Ok(u) => u,
        Err(_) => return,
    };
    // WebDAV DELETE on a collection removes the directory and all contents
    match webdav::delete_collection(&url, auth).await {
        Ok(true) => log::info!("[WebDAV] V1 remote data cleaned up"),
        Ok(false) => log::debug!("[WebDAV] V1 remote data already gone"),
        Err(e) => log::warn!("[WebDAV] Failed to clean up V1 remote data: {e}"),
    }
}

/// 迁移 V1 → V2：下载 V1 数据 → 本地应用 → 上传 V2 → 删除 V1
async fn migrate_v1_to_v2() -> Result<WebDavSyncSummary, AppError> {
    let settings = load_webdav_settings(true)?;
    let auth = webdav::auth_from_credentials(&settings.username, &settings.password);

    // 1. 下载 V1 manifest
    let v1_manifest = detect_v1_manifest(&settings, &auth).await?.ok_or_else(|| {
        localized(
            "webdav.sync.v1_not_found",
            "远端未找到 V1 同步数据",
            "No V1 sync data found on the remote",
        )
    })?;

    // 2. 下载 V1 artifacts（V1 的 settings_sync 不迁移，V2 不再同步该数据）
    let db_sql = download_v1_artifact(
        &settings,
        &auth,
        REMOTE_DB_SQL,
        &v1_manifest.artifacts.db_sql,
    )
    .await?;
    let skills_zip = download_v1_artifact(
        &settings,
        &auth,
        REMOTE_SKILLS_ZIP,
        &v1_manifest.artifacts.skills_zip,
    )
    .await?;

    // 3. 应用到本地
    apply_snapshot_with_restore_guard(&db_sql, &skills_zip).await?;

    // 4. 重新上传为 V2 格式（upload 内部会 best-effort 清理 V1 远端数据）
    upload().await?;

    Ok(WebDavSyncSummary {
        decision: SyncDecision::Download,
        message: "V1 → V2 migration completed".to_string(),
    })
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_settings() -> WebDavSyncSettings {
        WebDavSyncSettings {
            enabled: true,
            base_url: "https://dav.example.com/remote.php/dav/files/demo/".to_string(),
            remote_root: "cc switch-sync/team a".to_string(),
            profile: "default profile".to_string(),
            username: "demo".to_string(),
            password: "secret".to_string(),
            auto_sync: false,
            status: WebDavSyncStatus::default(),
        }
    }

    #[test]
    fn disabled_settings_can_be_loaded_for_connection_checks_only() {
        let temp = tempfile::tempdir().expect("create isolated home");
        let _environment = crate::test_support::TestEnvGuard::isolated(temp.path());
        let mut settings = sample_settings();
        settings.enabled = false;
        crate::settings::set_webdav_sync_settings(Some(settings))
            .expect("save disabled WebDAV settings");

        assert!(load_webdav_settings(false).is_ok());
        assert!(load_webdav_settings(true).is_err());
    }

    #[test]
    fn remote_dir_segments_uses_current_layout() {
        let mut settings = sample_settings();
        settings.normalize();
        let segments = remote_dir_segments(&settings, RemoteLayout::Current);
        assert_eq!(
            segments,
            vec![
                "cc switch-sync".to_string(),
                "team a".to_string(),
                "v2".to_string(),
                "db-v6".to_string(),
                "default profile".to_string(),
            ]
        );
    }

    #[test]
    fn build_artifact_url_encodes_path_segments() {
        let mut settings = sample_settings();
        settings.normalize();
        let url = build_artifact_url(&settings, RemoteLayout::Current, REMOTE_MANIFEST)
            .expect("build artifact url");
        assert_eq!(
            url,
            "https://dav.example.com/remote.php/dav/files/demo/cc%20switch-sync/team%20a/v2/db-v6/default%20profile/manifest.json"
        );
        assert!(
            !url.contains("//cc"),
            "url should not contain duplicated slash: {url}"
        );
    }

    #[test]
    fn snapshot_id_is_stable() {
        let mut artifacts = BTreeMap::new();
        artifacts.insert(
            "db.sql".to_string(),
            ArtifactMeta {
                sha256: "aaa".to_string(),
                size: 1,
            },
        );
        artifacts.insert(
            "skills.zip".to_string(),
            ArtifactMeta {
                sha256: "bbb".to_string(),
                size: 2,
            },
        );
        let id1 = compute_snapshot_id(&artifacts);
        let id2 = compute_snapshot_id(&artifacts);
        assert_eq!(id1, id2);
    }

    #[test]
    fn snapshot_id_changes_with_artifacts() {
        let mut artifacts_a = BTreeMap::new();
        artifacts_a.insert(
            "db.sql".to_string(),
            ArtifactMeta {
                sha256: "aaa".to_string(),
                size: 1,
            },
        );
        artifacts_a.insert(
            "skills.zip".to_string(),
            ArtifactMeta {
                sha256: "bbb".to_string(),
                size: 2,
            },
        );

        let mut artifacts_b = artifacts_a.clone();
        artifacts_b.get_mut("db.sql").unwrap().sha256 = "ccc".to_string();

        assert_ne!(
            compute_snapshot_id(&artifacts_a),
            compute_snapshot_id(&artifacts_b)
        );
    }

    fn manifest_with(format: &str, version: u32, db_compat_version: Option<u32>) -> SyncManifest {
        SyncManifest {
            format: format.to_string(),
            version,
            db_compat_version,
            device_name: "test".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            artifacts: BTreeMap::new(),
            snapshot_id: "id".to_string(),
        }
    }

    #[test]
    fn validate_manifest_compat_accepts_supported_manifest() {
        let manifest = manifest_with(PROTOCOL_FORMAT, PROTOCOL_VERSION, Some(DB_COMPAT_VERSION));
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Current).is_ok());
    }

    #[test]
    fn validate_manifest_compat_wrong_format() {
        let manifest = manifest_with("wrong-format", PROTOCOL_VERSION, Some(DB_COMPAT_VERSION));
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Current).is_err());
    }

    #[test]
    fn validate_manifest_compat_wrong_version() {
        let manifest = manifest_with(PROTOCOL_FORMAT, 999, Some(DB_COMPAT_VERSION));
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Current).is_err());
    }

    #[test]
    fn validate_manifest_compat_rejects_current_manifest_with_wrong_db_compat() {
        let manifest = manifest_with(PROTOCOL_FORMAT, PROTOCOL_VERSION, Some(5));
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Current).is_err());
    }

    #[test]
    fn remote_dir_segments_uses_legacy_layout() {
        let mut settings = sample_settings();
        settings.normalize();
        let segments = remote_dir_segments(&settings, RemoteLayout::Legacy);
        assert_eq!(
            segments,
            vec![
                "cc switch-sync".to_string(),
                "team a".to_string(),
                "v2".to_string(),
                "default profile".to_string(),
            ]
        );
    }

    #[test]
    fn validate_manifest_compat_accepts_legacy_manifest_without_db_compat() {
        let manifest = manifest_with(PROTOCOL_FORMAT, PROTOCOL_VERSION, None);
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Legacy).is_ok());
    }

    #[test]
    fn validate_manifest_compat_rejects_legacy_manifest_from_newer_db_generation() {
        let manifest = manifest_with(
            PROTOCOL_FORMAT,
            PROTOCOL_VERSION,
            Some(DB_COMPAT_VERSION + 1),
        );
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Legacy).is_err());
    }

    #[test]
    fn effective_db_compat_version_defaults_legacy_layout_to_v5() {
        let manifest = manifest_with(PROTOCOL_FORMAT, PROTOCOL_VERSION, None);
        assert_eq!(
            effective_db_compat_version(&manifest, RemoteLayout::Legacy),
            Some(LEGACY_DB_COMPAT_VERSION)
        );
        assert_eq!(
            effective_db_compat_version(&manifest, RemoteLayout::Current),
            None
        );
    }

    #[test]
    fn validate_artifact_size_limit_ok() {
        assert!(validate_artifact_size_limit("db.sql", 1024).is_ok());
    }

    #[test]
    fn validate_artifact_size_limit_exceeded() {
        assert!(validate_artifact_size_limit("db.sql", MAX_SYNC_ARTIFACT_BYTES + 1).is_err());
    }

    #[test]
    fn extract_sql_user_version_reads_pragma_and_comment() {
        assert_eq!(
            extract_sql_user_version("-- header\nPRAGMA user_version=10;\n"),
            Some(10)
        );
        assert_eq!(
            extract_sql_user_version("-- user_version: 11\nPRAGMA foreign_keys=OFF;\n"),
            Some(11)
        );
    }

    #[test]
    fn validate_sql_user_version_rejects_future_schema_before_restore() {
        let sql = format!(
            "-- CC Switch SQLite 导出\nPRAGMA user_version={};\n",
            SCHEMA_VERSION + 1
        );
        let err = validate_sql_user_version_for_import(&sql)
            .expect_err("future schema should be rejected before applying snapshot");
        assert!(
            err.to_string().contains("版本过新") || err.to_string().contains("schema is too new"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn apply_snapshot_accepts_current_schema_v13_sync_export() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());

        let remote_db = Database::memory().expect("create remote db");
        {
            let conn = crate::database::lock_conn!(remote_db.conn);
            Database::set_user_version(&conn, SCHEMA_VERSION)
                .expect("mark remote snapshot as current schema");
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config, meta)
                 VALUES ('remote-provider', 'claude', 'Remote Provider', '{}', '{}')",
                [],
            )
            .expect("insert remote provider");
        }
        let db_sql = remote_db
            .export_sql_string_for_sync()
            .expect("export v13 sync sql");

        let zip_path = temp.path().join("remote-skills.zip");
        {
            let file = std::fs::File::create(&zip_path).expect("create remote skills zip");
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file(
                    "remote-skill/SKILL.md",
                    crate::services::webdav_sync::archive::zip_file_options(),
                )
                .expect("start remote skill");
            use std::io::Write;
            writer.write_all(b"remote").expect("write remote skill");
            writer.finish().expect("finish remote skills zip");
        }
        let skills_zip = std::fs::read(&zip_path).expect("read remote skills zip");

        apply_snapshot(db_sql.as_bytes(), &skills_zip).expect("apply current-schema snapshot");

        let local_db = Database::init().expect("open local db");
        let conn = crate::database::lock_conn!(local_db.conn);
        let provider_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM providers WHERE id = 'remote-provider'",
                [],
                |row| row.get(0),
            )
            .expect("count imported provider");
        assert_eq!(provider_count, 1);
        assert!(
            crate::services::skill::SkillService::get_ssot_dir()
                .expect("ssot dir")
                .join("remote-skill")
                .join("SKILL.md")
                .exists(),
            "current-schema restore should unpack remote skills"
        );
        Ok(())
    }

    #[test]
    fn apply_snapshot_rejects_future_schema_without_touching_existing_skills() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());

        let existing_skill = crate::services::skill::SkillService::get_ssot_dir()
            .expect("ssot dir")
            .join("existing")
            .join("SKILL.md");
        std::fs::create_dir_all(existing_skill.parent().expect("skill parent"))
            .expect("create existing skill parent");
        std::fs::write(&existing_skill, "existing").expect("write existing skill");

        let zip_path = temp.path().join("replacement-skills.zip");
        {
            let file = std::fs::File::create(&zip_path).expect("create replacement zip");
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file(
                    "replacement/SKILL.md",
                    crate::services::webdav_sync::archive::zip_file_options(),
                )
                .expect("start replacement skill");
            use std::io::Write;
            writer
                .write_all(b"replacement")
                .expect("write replacement skill");
            writer.finish().expect("finish replacement zip");
        }
        let skills_zip = std::fs::read(&zip_path).expect("read replacement zip");
        let sql = format!(
            "-- CC Switch SQLite 导出\nPRAGMA user_version={};\n",
            SCHEMA_VERSION + 1
        );

        apply_snapshot(sql.as_bytes(), &skills_zip)
            .expect_err("future schema should be rejected before restoring skills");

        assert_eq!(
            std::fs::read_to_string(&existing_skill).expect("read existing skill"),
            "existing",
            "future schema restore must leave existing skills untouched"
        );
        assert!(
            !crate::services::skill::SkillService::get_ssot_dir()
                .expect("ssot dir")
                .join("replacement")
                .exists(),
            "future schema restore must not unpack replacement skills"
        );
    }

    #[cfg(unix)]
    #[test]
    fn apply_snapshot_rolls_back_skills_when_database_init_fails() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("create temp dir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());

        let ssot = crate::services::skill::SkillService::get_ssot_dir().expect("ssot dir");
        let existing_skill = ssot.join("existing").join("SKILL.md");
        std::fs::create_dir_all(existing_skill.parent().expect("skill parent"))
            .expect("create existing skill parent");
        std::fs::write(&existing_skill, "existing").expect("write existing skill");

        let zip_path = temp.path().join("replacement-skills.zip");
        {
            let file = std::fs::File::create(&zip_path).expect("create replacement zip");
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file(
                    "replacement/SKILL.md",
                    crate::services::webdav_sync::archive::zip_file_options(),
                )
                .expect("start replacement skill");
            use std::io::Write;
            writer
                .write_all(b"replacement")
                .expect("write replacement skill");
            writer.finish().expect("finish replacement zip");
        }
        let skills_zip = std::fs::read(&zip_path).expect("read replacement zip");

        let db_target = temp.path().join("external.db");
        std::fs::write(&db_target, b"not sqlite").expect("write external db");
        let db_link = temp.path().join(".cc-switch").join("cc-switch.db");
        symlink(&db_target, &db_link).expect("create db symlink");

        apply_snapshot(
            b"-- CC Switch SQLite export\nPRAGMA user_version=0;\n",
            &skills_zip,
        )
        .expect_err("db init failure should fail the restore");

        assert_eq!(
            std::fs::read_to_string(&existing_skill).expect("read existing skill"),
            "existing",
            "DB init failure must roll back restored skills"
        );
        assert!(
            !ssot.join("replacement").exists(),
            "DB init failure must remove replacement skills"
        );
    }

    #[cfg(unix)]
    #[test]
    fn apply_snapshot_rejects_symlink_parent_config_dir_before_restoring_skills() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("create temp dir");
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        let real_parent = temp.path().join("real-parent");
        let external_parent = temp.path().join("external-parent");
        std::fs::create_dir(&real_parent).expect("create real parent");
        std::fs::create_dir(&external_parent).expect("create external parent");
        symlink(&external_parent, real_parent.join("link")).expect("create symlink parent");
        unsafe {
            std::env::set_var(
                "CC_SWITCH_CONFIG_DIR",
                real_parent.join("link").join("..").join("cc-switch"),
            );
        }

        let zip_path = temp.path().join("replacement-skills.zip");
        {
            let file = std::fs::File::create(&zip_path).expect("create replacement zip");
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file(
                    "replacement/SKILL.md",
                    crate::services::webdav_sync::archive::zip_file_options(),
                )
                .expect("start replacement skill");
            use std::io::Write;
            writer
                .write_all(b"replacement")
                .expect("write replacement skill");
            writer.finish().expect("finish replacement zip");
        }
        let skills_zip = std::fs::read(&zip_path).expect("read replacement zip");

        let err = apply_snapshot(
            b"-- CC Switch SQLite export\nPRAGMA user_version=0;\n",
            &skills_zip,
        )
        .expect_err("symlink parent config dir should fail before restoring skills");

        assert!(
            err.to_string().contains("符号链接") || err.to_string().contains("symlink"),
            "unexpected error: {err}"
        );
        assert!(
            !real_parent.join("cc-switch/skills").exists(),
            "restore must not create the normalized skills directory"
        );
        assert!(
            !external_parent.join("cc-switch/skills").exists(),
            "restore must not follow the symlinked parent into external storage"
        );
    }

    #[test]
    fn normalize_device_name_trims() {
        assert_eq!(
            normalize_device_name("  my-host  "),
            Some("my-host".to_string())
        );
    }

    #[test]
    fn normalize_device_name_empty() {
        assert_eq!(normalize_device_name(""), None);
        assert_eq!(normalize_device_name("   "), None);
    }

    #[test]
    fn normalize_device_name_truncates() {
        let long = "a".repeat(100);
        let result = normalize_device_name(&long).unwrap();
        assert_eq!(result.chars().count(), MAX_DEVICE_NAME_LEN);
    }

    #[test]
    fn normalize_device_name_collapses_whitespace() {
        assert_eq!(
            normalize_device_name("  Mac  Book  Pro  "),
            Some("Mac Book Pro".to_string())
        );
    }

    #[test]
    fn normalize_device_name_truncates_by_chars_not_bytes() {
        // 中文字符每个 3 bytes，80 个中文 = 240 bytes
        let long_cn = "测".repeat(80);
        let result = normalize_device_name(&long_cn).unwrap();
        assert_eq!(result.chars().count(), MAX_DEVICE_NAME_LEN);
    }

    #[test]
    fn detect_system_device_name_returns_env_name() {
        std::env::set_var("CC_SWITCH_DEVICE_NAME", "test-device");
        let name = detect_system_device_name();
        std::env::remove_var("CC_SWITCH_DEVICE_NAME");
        assert_eq!(name.as_deref(), Some("test-device"));
    }

    #[test]
    fn manifest_serialization_uses_device_name_only() {
        let manifest = SyncManifest {
            format: PROTOCOL_FORMAT.to_string(),
            version: PROTOCOL_VERSION,
            db_compat_version: Some(DB_COMPAT_VERSION),
            device_name: "My MacBook".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            artifacts: BTreeMap::new(),
            snapshot_id: "snap-1".to_string(),
        };
        let value = serde_json::to_value(&manifest).expect("serialize manifest");
        assert!(
            value.get("deviceName").is_some(),
            "manifest should contain deviceName"
        );
        assert!(
            value.get("deviceId").is_none(),
            "manifest should not contain deviceId"
        );
        assert_eq!(
            value.get("dbCompatVersion").and_then(|v| v.as_u64()),
            Some(DB_COMPAT_VERSION as u64)
        );
    }
}
