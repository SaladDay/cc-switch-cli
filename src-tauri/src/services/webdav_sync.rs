use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use url::Url;
use zip::{write::SimpleFileOptions, DateTime};

use crate::config::atomic_write;
use crate::database::Database;
use crate::error::AppError;
use crate::services::memory::MemoryService;
use crate::services::skill::SkillService;
use crate::settings::{
    get_settings, get_webdav_sync_settings, set_webdav_sync_settings, update_settings,
    CustomEndpoint, SecuritySettings, WebDavSyncSettings,
};

const PROTOCOL_FORMAT: &str = "cc-switch-webdav-sync";
const PROTOCOL_VERSION: u32 = 1;
const REMOTE_DB_SQL: &str = "db.sql";
const REMOTE_SKILLS_ZIP: &str = "skills.zip";
const REMOTE_SETTINGS_SYNC: &str = "settings.sync.json";
const REMOTE_MANIFEST: &str = "manifest.json";
const REMOTE_CLAUDE_ZIP: &str = "claude.zip";
const REMOTE_CODEX_ZIP: &str = "codex.zip";
const REMOTE_GEMINI_ZIP: &str = "gemini.zip";
const REMOTE_MEMORY_SQL: &str = "memory.sql";

const CLAUDE_EXCLUDES: &[&str] = &[
    "debug", "cache", "paste-cache", "telemetry", "statsig",
    "session-env", "shell-snapshots", "tasks", "plugins",
    "usage-data", "stats-cache.json", "statusline-command.sh",
];
const CODEX_EXCLUDES: &[&str] = &["log", "tmp"];
const GEMINI_EXCLUDES: &[&str] = &[
    "antigravity", "antigravity-browser-profile",
    "oauth_creds.json", "google_accounts.json", "tmp",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDecision {
    Upload,
    Download,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebDavSyncSummary {
    pub decision: SyncDecision,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestArtifact {
    path: String,
    sha256: String,
    size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebDavManifest {
    format: String,
    version: u32,
    updated_at: String,
    updated_by: String,
    artifacts: ManifestArtifacts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestArtifacts {
    db_sql: ManifestArtifact,
    skills_zip: ManifestArtifact,
    settings_sync: ManifestArtifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claude_zip: Option<ManifestArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codex_zip: Option<ManifestArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gemini_zip: Option<ManifestArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memory_sql: Option<ManifestArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncableSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    skill_sync_method: crate::services::skill::SyncMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    security: Option<SecuritySettings>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    custom_endpoints_claude: BTreeMap<String, CustomEndpoint>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    custom_endpoints_codex: BTreeMap<String, CustomEndpoint>,
}

struct LocalSnapshot {
    db_sql: Vec<u8>,
    skills_zip: Vec<u8>,
    settings_sync: Vec<u8>,
    claude_zip: Option<Vec<u8>>,
    codex_zip: Option<Vec<u8>>,
    gemini_zip: Option<Vec<u8>>,
    memory_sql: Option<Vec<u8>>,
    manifest: WebDavManifest,
    manifest_bytes: Vec<u8>,
    manifest_hash: String,
}

struct RemoteManifestState {
    manifest: WebDavManifest,
    manifest_hash: String,
    etag: Option<String>,
}

pub struct WebDavSyncService;

impl WebDavSyncService {
    pub fn check_connection() -> Result<(), AppError> {
        let settings = load_webdav_settings()?;
        ensure_remote_dirs(&settings)?;
        let _ = fetch_remote_manifest(&settings)?;
        Ok(())
    }

    pub fn upload() -> Result<WebDavSyncSummary, AppError> {
        let mut settings = load_webdav_settings()?;
        ensure_remote_dirs(&settings)?;
        let snapshot = build_local_snapshot(&settings)?;
        upload_snapshot(&settings, &snapshot)?;

        settings.status.last_sync_at = Some(Utc::now().timestamp());
        settings.status.last_error = None;
        settings.status.last_local_manifest_hash = Some(snapshot.manifest_hash.clone());
        settings.status.last_remote_manifest_hash = Some(snapshot.manifest_hash.clone());
        settings.status.last_remote_etag = head_remote_etag(&settings, REMOTE_MANIFEST)?;
        set_webdav_sync_settings(Some(settings))?;

        Ok(WebDavSyncSummary {
            decision: SyncDecision::Upload,
            message: "WebDAV upload completed".to_string(),
        })
    }

    pub fn download() -> Result<WebDavSyncSummary, AppError> {
        let mut settings = load_webdav_settings()?;
        ensure_remote_dirs(&settings)?;
        let remote = fetch_remote_manifest(&settings)?.ok_or_else(|| {
            AppError::InvalidInput("远端没有可下载的 WebDAV manifest".to_string())
        })?;

        let db_sql = download_and_verify_artifact(&settings, &remote.manifest.artifacts.db_sql)?;
        let skills_zip =
            download_and_verify_artifact(&settings, &remote.manifest.artifacts.skills_zip)?;
        let settings_sync =
            download_and_verify_artifact(&settings, &remote.manifest.artifacts.settings_sync)?;

        let claude_zip = remote
            .manifest
            .artifacts
            .claude_zip
            .as_ref()
            .map(|a| download_and_verify_artifact(&settings, a))
            .transpose()?;
        let codex_zip = remote
            .manifest
            .artifacts
            .codex_zip
            .as_ref()
            .map(|a| download_and_verify_artifact(&settings, a))
            .transpose()?;
        let gemini_zip = remote
            .manifest
            .artifacts
            .gemini_zip
            .as_ref()
            .map(|a| download_and_verify_artifact(&settings, a))
            .transpose()?;
        let memory_sql = remote
            .manifest
            .artifacts
            .memory_sql
            .as_ref()
            .map(|a| download_and_verify_artifact(&settings, a))
            .transpose()?;

        apply_downloaded_snapshot(
            &db_sql,
            &skills_zip,
            &settings_sync,
            claude_zip.as_deref(),
            codex_zip.as_deref(),
            gemini_zip.as_deref(),
            memory_sql.as_deref(),
        )?;

        settings.status.last_sync_at = Some(Utc::now().timestamp());
        settings.status.last_error = None;
        settings.status.last_local_manifest_hash = Some(remote.manifest_hash.clone());
        settings.status.last_remote_manifest_hash = Some(remote.manifest_hash);
        settings.status.last_remote_etag = remote.etag;
        set_webdav_sync_settings(Some(settings))?;

        Ok(WebDavSyncSummary {
            decision: SyncDecision::Download,
            message: "WebDAV download completed".to_string(),
        })
    }
}

fn load_webdav_settings() -> Result<WebDavSyncSettings, AppError> {
    let settings = get_webdav_sync_settings()
        .ok_or_else(|| AppError::InvalidInput("未配置 WebDAV 同步".to_string()))?;
    if !settings.enabled {
        return Err(AppError::InvalidInput("WebDAV 同步未启用".to_string()));
    }
    settings.validate()?;
    Ok(settings)
}

fn build_local_snapshot(settings: &WebDavSyncSettings) -> Result<LocalSnapshot, AppError> {
    let tmp = tempdir().map_err(|e| AppError::IoContext {
        context: "创建 WebDAV 快照临时目录失败".to_string(),
        source: e,
    })?;

    let db_path = tmp.path().join(REMOTE_DB_SQL);
    Database::init()?.export_sql(&db_path)?;
    let db_sql = fs::read(&db_path).map_err(|e| AppError::io(&db_path, e))?;

    let syncable = to_syncable_settings();
    let settings_sync =
        serde_json::to_vec_pretty(&syncable).map_err(|e| AppError::JsonSerialize { source: e })?;
    let settings_path = tmp.path().join(REMOTE_SETTINGS_SYNC);
    atomic_write(&settings_path, &settings_sync)?;

    let skills_zip_path = tmp.path().join(REMOTE_SKILLS_ZIP);
    zip_skills_ssot(&skills_zip_path)?;
    let skills_zip = fs::read(&skills_zip_path).map_err(|e| AppError::io(&skills_zip_path, e))?;

    // CLI directories
    let claude_zip = zip_cli_dir(&crate::config::get_claude_config_dir(), CLAUDE_EXCLUDES)?;
    let codex_zip = zip_cli_dir(&crate::codex_config::get_codex_config_dir(), CODEX_EXCLUDES)?;
    let gemini_zip = zip_cli_dir(&crate::gemini_config::get_gemini_dir(), GEMINI_EXCLUDES)?;

    // Memory database
    let memory_sql = MemoryService::export_sql_bytes()?;

    let db_sql_hash = sha256_hex(&db_sql);
    let skills_zip_hash = sha256_hex(&skills_zip);
    let settings_sync_hash = sha256_hex(&settings_sync);
    let claude_zip_hash = claude_zip.as_ref().map(|b| sha256_hex(b));
    let codex_zip_hash = codex_zip.as_ref().map(|b| sha256_hex(b));
    let gemini_zip_hash = gemini_zip.as_ref().map(|b| sha256_hex(b));
    let memory_sql_hash = memory_sql.as_ref().map(|b| sha256_hex(b));

    let manifest = WebDavManifest {
        format: PROTOCOL_FORMAT.to_string(),
        version: PROTOCOL_VERSION,
        updated_at: Utc::now().to_rfc3339(),
        updated_by: settings.device_id.clone(),
        artifacts: ManifestArtifacts {
            db_sql: ManifestArtifact {
                path: REMOTE_DB_SQL.to_string(),
                sha256: db_sql_hash.clone(),
                size: db_sql.len() as u64,
            },
            skills_zip: ManifestArtifact {
                path: REMOTE_SKILLS_ZIP.to_string(),
                sha256: skills_zip_hash.clone(),
                size: skills_zip.len() as u64,
            },
            settings_sync: ManifestArtifact {
                path: REMOTE_SETTINGS_SYNC.to_string(),
                sha256: settings_sync_hash.clone(),
                size: settings_sync.len() as u64,
            },
            claude_zip: claude_zip.as_ref().map(|b| ManifestArtifact {
                path: REMOTE_CLAUDE_ZIP.to_string(),
                sha256: claude_zip_hash.clone().unwrap(),
                size: b.len() as u64,
            }),
            codex_zip: codex_zip.as_ref().map(|b| ManifestArtifact {
                path: REMOTE_CODEX_ZIP.to_string(),
                sha256: codex_zip_hash.clone().unwrap(),
                size: b.len() as u64,
            }),
            gemini_zip: gemini_zip.as_ref().map(|b| ManifestArtifact {
                path: REMOTE_GEMINI_ZIP.to_string(),
                sha256: gemini_zip_hash.clone().unwrap(),
                size: b.len() as u64,
            }),
            memory_sql: memory_sql.as_ref().map(|b| ManifestArtifact {
                path: REMOTE_MEMORY_SQL.to_string(),
                sha256: memory_sql_hash.clone().unwrap(),
                size: b.len() as u64,
            }),
        },
    };
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|e| AppError::JsonSerialize { source: e })?;
    let manifest_hash = snapshot_identity_from_hashes(
        &db_sql_hash,
        &skills_zip_hash,
        &settings_sync_hash,
        claude_zip_hash.as_deref(),
        codex_zip_hash.as_deref(),
        gemini_zip_hash.as_deref(),
        memory_sql_hash.as_deref(),
    );

    Ok(LocalSnapshot {
        db_sql,
        skills_zip,
        settings_sync,
        claude_zip,
        codex_zip,
        gemini_zip,
        memory_sql,
        manifest,
        manifest_bytes,
        manifest_hash,
    })
}

fn to_syncable_settings() -> SyncableSettings {
    let settings = get_settings();
    SyncableSettings {
        language: settings.language,
        skill_sync_method: settings.skill_sync_method,
        security: settings.security,
        custom_endpoints_claude: settings.custom_endpoints_claude.into_iter().collect(),
        custom_endpoints_codex: settings.custom_endpoints_codex.into_iter().collect(),
    }
}

fn apply_downloaded_snapshot(
    db_sql: &[u8],
    skills_zip: &[u8],
    settings_sync: &[u8],
    claude_zip: Option<&[u8]>,
    codex_zip: Option<&[u8]>,
    gemini_zip: Option<&[u8]>,
    memory_sql: Option<&[u8]>,
) -> Result<(), AppError> {
    let tmp = tempdir().map_err(|e| AppError::IoContext {
        context: "创建 WebDAV 下载临时目录失败".to_string(),
        source: e,
    })?;

    let db_path = tmp.path().join(REMOTE_DB_SQL);
    atomic_write(&db_path, db_sql)?;
    Database::init()?.import_sql(&db_path)?;

    apply_syncable_settings(settings_sync)?;
    restore_skills_zip(skills_zip)?;

    // Restore CLI directories (merge mode)
    if let Some(bytes) = claude_zip {
        restore_cli_zip(bytes, &crate::config::get_claude_config_dir())?;
    }
    if let Some(bytes) = codex_zip {
        restore_cli_zip(bytes, &crate::codex_config::get_codex_config_dir())?;
    }
    if let Some(bytes) = gemini_zip {
        restore_cli_zip(bytes, &crate::gemini_config::get_gemini_dir())?;
    }

    // Restore memory
    if let Some(bytes) = memory_sql {
        MemoryService::import_sql_bytes(bytes)?;
    }

    Ok(())
}

fn apply_syncable_settings(raw: &[u8]) -> Result<(), AppError> {
    let incoming: SyncableSettings = serde_json::from_slice(raw).map_err(|e| AppError::Json {
        path: REMOTE_SETTINGS_SYNC.to_string(),
        source: e,
    })?;

    let mut settings = get_settings();
    settings.language = incoming.language;
    settings.skill_sync_method = incoming.skill_sync_method;
    settings.security = incoming.security;
    settings.custom_endpoints_claude = incoming.custom_endpoints_claude.into_iter().collect();
    settings.custom_endpoints_codex = incoming.custom_endpoints_codex.into_iter().collect();
    update_settings(settings)
}

fn restore_skills_zip(raw: &[u8]) -> Result<(), AppError> {
    let tmp = tempdir().map_err(|e| AppError::IoContext {
        context: "创建 skills 解压临时目录失败".to_string(),
        source: e,
    })?;
    let zip_path = tmp.path().join(REMOTE_SKILLS_ZIP);
    atomic_write(&zip_path, raw)?;

    let file = fs::File::open(&zip_path).map_err(|e| AppError::io(&zip_path, e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| AppError::Message(format!("解析 skills.zip 失败: {e}")))?;

    let extracted = tmp.path().join("skills-extracted");
    fs::create_dir_all(&extracted).map_err(|e| AppError::io(&extracted, e))?;

    for idx in 0..archive.len() {
        let mut entry = archive
            .by_index(idx)
            .map_err(|e| AppError::Message(format!("读取 ZIP 项失败: {e}")))?;
        let Some(safe_name) = entry.enclosed_name() else {
            continue;
        };
        let out_path = extracted.join(safe_name);
        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| AppError::io(&out_path, e))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
        let mut out = fs::File::create(&out_path).map_err(|e| AppError::io(&out_path, e))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| AppError::io(&out_path, e))?;
    }

    let ssot = SkillService::get_ssot_dir()?;
    if ssot.exists() {
        fs::remove_dir_all(&ssot).map_err(|e| AppError::io(&ssot, e))?;
    }
    fs::create_dir_all(&ssot).map_err(|e| AppError::io(&ssot, e))?;
    copy_dir_recursive(&extracted, &ssot)?;
    Ok(())
}

fn upload_snapshot(
    settings: &WebDavSyncSettings,
    snapshot: &LocalSnapshot,
) -> Result<(), AppError> {
    ensure_remote_dirs(settings)?;
    put_remote_bytes(settings, REMOTE_DB_SQL, &snapshot.db_sql, "application/sql")?;
    put_remote_bytes(
        settings,
        REMOTE_SETTINGS_SYNC,
        &snapshot.settings_sync,
        "application/json",
    )?;
    put_remote_bytes(
        settings,
        REMOTE_SKILLS_ZIP,
        &snapshot.skills_zip,
        "application/zip",
    )?;

    if let Some(ref bytes) = snapshot.claude_zip {
        put_remote_bytes(settings, REMOTE_CLAUDE_ZIP, bytes, "application/zip")?;
    }
    if let Some(ref bytes) = snapshot.codex_zip {
        put_remote_bytes(settings, REMOTE_CODEX_ZIP, bytes, "application/zip")?;
    }
    if let Some(ref bytes) = snapshot.gemini_zip {
        put_remote_bytes(settings, REMOTE_GEMINI_ZIP, bytes, "application/zip")?;
    }
    if let Some(ref bytes) = snapshot.memory_sql {
        put_remote_bytes(settings, REMOTE_MEMORY_SQL, bytes, "application/sql")?;
    }

    let _ = &snapshot.manifest;
    put_remote_bytes(
        settings,
        REMOTE_MANIFEST,
        &snapshot.manifest_bytes,
        "application/json",
    )?;
    Ok(())
}

fn download_and_verify_artifact(
    settings: &WebDavSyncSettings,
    artifact: &ManifestArtifact,
) -> Result<Vec<u8>, AppError> {
    let (bytes, _) = get_remote_bytes(settings, &artifact.path)?
        .ok_or_else(|| AppError::Message(format!("远端缺少 artifact: {}", artifact.path)))?;
    let hash = sha256_hex(&bytes);
    if hash != artifact.sha256 {
        return Err(AppError::Message(format!(
            "artifact hash mismatch for {}",
            artifact.path
        )));
    }
    Ok(bytes)
}

fn fetch_remote_manifest(
    settings: &WebDavSyncSettings,
) -> Result<Option<RemoteManifestState>, AppError> {
    let Some((bytes, etag)) = get_remote_bytes(settings, REMOTE_MANIFEST)? else {
        return Ok(None);
    };
    let manifest: WebDavManifest = serde_json::from_slice(&bytes).map_err(|e| AppError::Json {
        path: REMOTE_MANIFEST.to_string(),
        source: e,
    })?;
    if manifest.format != PROTOCOL_FORMAT || manifest.version != PROTOCOL_VERSION {
        return Err(AppError::InvalidInput(
            "远端 manifest 协议不兼容".to_string(),
        ));
    }
    let manifest_hash = snapshot_identity_from_manifest(&manifest);
    Ok(Some(RemoteManifestState {
        manifest_hash,
        manifest,
        etag,
    }))
}

fn snapshot_identity_from_manifest(manifest: &WebDavManifest) -> String {
    snapshot_identity_from_hashes(
        &manifest.artifacts.db_sql.sha256,
        &manifest.artifacts.skills_zip.sha256,
        &manifest.artifacts.settings_sync.sha256,
        manifest.artifacts.claude_zip.as_ref().map(|a| a.sha256.as_str()),
        manifest.artifacts.codex_zip.as_ref().map(|a| a.sha256.as_str()),
        manifest.artifacts.gemini_zip.as_ref().map(|a| a.sha256.as_str()),
        manifest.artifacts.memory_sql.as_ref().map(|a| a.sha256.as_str()),
    )
}

fn snapshot_identity_from_hashes(
    db_hash: &str,
    skills_hash: &str,
    settings_hash: &str,
    claude_hash: Option<&str>,
    codex_hash: Option<&str>,
    gemini_hash: Option<&str>,
    memory_hash: Option<&str>,
) -> String {
    let mut combined = format!("{db_hash}:{skills_hash}:{settings_hash}");
    if let Some(h) = claude_hash {
        combined.push(':');
        combined.push_str(h);
    }
    if let Some(h) = codex_hash {
        combined.push(':');
        combined.push_str(h);
    }
    if let Some(h) = gemini_hash {
        combined.push(':');
        combined.push_str(h);
    }
    if let Some(h) = memory_hash {
        combined.push(':');
        combined.push_str(h);
    }
    sha256_hex(combined.as_bytes())
}

fn ensure_remote_dirs(settings: &WebDavSyncSettings) -> Result<(), AppError> {
    let mut current = Vec::<String>::new();
    for segment in remote_dir_segments(settings) {
        current.push(segment);
        ensure_remote_dir(settings, &current)?;
    }
    Ok(())
}

fn ensure_remote_dir(
    settings: &WebDavSyncSettings,
    rel_segments: &[String],
) -> Result<(), AppError> {
    match propfind_remote_dir(settings, rel_segments)? {
        RemoteDirProbe::Exists => return Ok(()),
        RemoteDirProbe::Missing | RemoteDirProbe::Unsupported => {}
    }

    let status = mkcol_remote_dir(settings, rel_segments)?;
    match status {
        StatusCode::CREATED => Ok(()),
        status if should_verify_after_mkcol(status) => {
            match propfind_remote_dir(settings, rel_segments)? {
                RemoteDirProbe::Exists => Ok(()),
                RemoteDirProbe::Missing | RemoteDirProbe::Unsupported => {
                    let url = build_remote_url(settings, rel_segments)?;
                    Err(webdav_status_error(settings, "MKCOL", status, &url))
                }
            }
        }
        _ => {
            let url = build_remote_url(settings, rel_segments)?;
            Err(webdav_status_error(settings, "MKCOL", status, &url))
        }
    }
}

fn should_verify_after_mkcol(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
            | StatusCode::CONFLICT
    )
}

fn remote_dir_segments(settings: &WebDavSyncSettings) -> Vec<String> {
    let mut segments = Vec::new();
    segments.extend(path_segments(&settings.remote_root).map(str::to_string));
    segments.push(format!("v{PROTOCOL_VERSION}"));
    segments.extend(path_segments(&settings.profile).map(str::to_string));
    segments
}

fn remote_file_segments(settings: &WebDavSyncSettings, file_name: &str) -> Vec<String> {
    let mut segments = remote_dir_segments(settings);
    segments.extend(path_segments(file_name).map(str::to_string));
    segments
}

fn path_segments(raw: &str) -> impl Iterator<Item = &str> {
    raw.trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
}

fn build_remote_url(
    settings: &WebDavSyncSettings,
    segments: &[String],
) -> Result<String, AppError> {
    let mut url = Url::parse(&settings.base_url)
        .map_err(|e| AppError::InvalidInput(format!("WebDAV base_url 不是合法 URL: {e}")))?;
    {
        let mut path_builder = url.path_segments_mut().map_err(|_| {
            AppError::InvalidInput("WebDAV base_url 必须是分层目录地址".to_string())
        })?;
        path_builder.pop_if_empty();
        for segment in segments {
            path_builder.push(segment);
        }
    }
    Ok(url.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteDirProbe {
    Exists,
    Missing,
    Unsupported,
}

fn propfind_remote_dir(
    settings: &WebDavSyncSettings,
    rel_segments: &[String],
) -> Result<RemoteDirProbe, AppError> {
    let url = build_remote_url(settings, rel_segments)?;
    let client = build_http_client(settings)?;
    let username = settings.username.clone();
    let password = settings.password.clone();

    run_http(async move {
        let method =
            Method::from_bytes(b"PROPFIND").map_err(|e| AppError::Message(e.to_string()))?;
        let resp = client
            .request(method, &url)
            .basic_auth(username, Some(password))
            .header("Depth", "0")
            .send()
            .await
            .map_err(|e| {
                AppError::Message(with_service_hint(
                    settings,
                    format!("WebDAV PROPFIND 请求失败: {e}"),
                ))
            })?;

        match resp.status() {
            StatusCode::OK | StatusCode::MULTI_STATUS | StatusCode::NO_CONTENT => {
                Ok(RemoteDirProbe::Exists)
            }
            StatusCode::NOT_FOUND => Ok(RemoteDirProbe::Missing),
            StatusCode::METHOD_NOT_ALLOWED => Ok(RemoteDirProbe::Unsupported),
            status => Err(webdav_status_error(settings, "PROPFIND", status, &url)),
        }
    })
}

fn mkcol_remote_dir(
    settings: &WebDavSyncSettings,
    rel_segments: &[String],
) -> Result<StatusCode, AppError> {
    let url = build_remote_url(settings, rel_segments)?;
    let client = build_http_client(settings)?;
    let username = settings.username.clone();
    let password = settings.password.clone();

    run_http(async move {
        let method = Method::from_bytes(b"MKCOL").map_err(|e| AppError::Message(e.to_string()))?;
        let resp = client
            .request(method, &url)
            .basic_auth(username, Some(password))
            .send()
            .await
            .map_err(|e| {
                AppError::Message(with_service_hint(
                    settings,
                    format!("WebDAV MKCOL 请求失败: {e}"),
                ))
            })?;
        Ok(resp.status())
    })
}

fn webdav_status_error(
    settings: &WebDavSyncSettings,
    operation: &str,
    status: StatusCode,
    url: &str,
) -> AppError {
    let mut message = format!("WebDAV {operation} 失败: {status} ({url})");
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        if is_jianguoyun(settings) {
            message.push_str(
                "。坚果云通常需要“第三方应用密码”，并且 base_url 应指向 /dav/ 下的目录。",
            );
        } else {
            message.push_str("。请检查 WebDAV 用户名、密码，以及该目录的读写权限。");
        }
    } else if is_jianguoyun(settings)
        && (status == StatusCode::NOT_FOUND || status.is_redirection())
    {
        message.push_str(
            "。坚果云常见原因是 base_url 不在 /dav/ 可写目录下；请改为 https://dav.jianguoyun.com/dav/....",
        );
    } else if operation == "MKCOL" && status == StatusCode::CONFLICT {
        if is_jianguoyun(settings) {
            message
                .push_str("。坚果云对分层自动建目录较敏感，请先在服务端手动创建上级目录后再重试。");
        } else {
            message.push_str("。请确认上级目录存在，或将 remote_root/profile 调整到可写路径。");
        }
    } else if operation == "MKCOL" && status == StatusCode::METHOD_NOT_ALLOWED {
        message.push_str("。目录可能已存在，可忽略此状态。");
    }
    AppError::Message(message)
}

fn is_jianguoyun(settings: &WebDavSyncSettings) -> bool {
    Url::parse(&settings.base_url)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_lowercase()))
        .map(|host| host.contains("jianguoyun.com") || host.contains("nutstore"))
        .unwrap_or(false)
}

fn with_service_hint(settings: &WebDavSyncSettings, message: impl Into<String>) -> String {
    let mut msg = message.into();
    if is_jianguoyun(settings) {
        msg.push_str(
            "。坚果云请优先使用“第三方应用密码”，并确认 base_url 指向 /dav/ 下的可写目录。",
        );
    }
    msg
}

fn put_remote_bytes(
    settings: &WebDavSyncSettings,
    file_name: &str,
    bytes: &[u8],
    content_type: &str,
) -> Result<(), AppError> {
    let rel_segments = remote_file_segments(settings, file_name);
    let url = build_remote_url(settings, &rel_segments)?;
    let body = bytes.to_vec();
    let client = build_http_client(settings)?;
    let username = settings.username.clone();
    let password = settings.password.clone();
    run_http(async move {
        let resp = client
            .put(&url)
            .basic_auth(username, Some(password))
            .header("Content-Type", content_type)
            .body(body)
            .send()
            .await
            .map_err(|e| {
                AppError::Message(with_service_hint(
                    settings,
                    format!("WebDAV PUT 请求失败: {e}"),
                ))
            })?;
        if !resp.status().is_success() {
            return Err(webdav_status_error(settings, "PUT", resp.status(), &url));
        }
        Ok(())
    })
}

fn get_remote_bytes(
    settings: &WebDavSyncSettings,
    file_name: &str,
) -> Result<Option<(Vec<u8>, Option<String>)>, AppError> {
    let rel_segments = remote_file_segments(settings, file_name);
    let url = build_remote_url(settings, &rel_segments)?;
    let client = build_http_client(settings)?;
    let username = settings.username.clone();
    let password = settings.password.clone();
    run_http(async move {
        let resp = client
            .get(&url)
            .basic_auth(username, Some(password))
            .send()
            .await
            .map_err(|e| {
                AppError::Message(with_service_hint(
                    settings,
                    format!("WebDAV GET 请求失败: {e}"),
                ))
            })?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(webdav_status_error(settings, "GET", resp.status(), &url));
        }
        let etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AppError::Message(format!("读取 WebDAV 响应失败: {e}")))?;
        Ok(Some((bytes.to_vec(), etag)))
    })
}

fn head_remote_etag(
    settings: &WebDavSyncSettings,
    file_name: &str,
) -> Result<Option<String>, AppError> {
    let rel_segments = remote_file_segments(settings, file_name);
    let url = build_remote_url(settings, &rel_segments)?;
    let client = build_http_client(settings)?;
    let username = settings.username.clone();
    let password = settings.password.clone();
    run_http(async move {
        let resp = client
            .head(&url)
            .basic_auth(username, Some(password))
            .send()
            .await
            .map_err(|e| {
                AppError::Message(with_service_hint(
                    settings,
                    format!("WebDAV HEAD 请求失败: {e}"),
                ))
            })?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(webdav_status_error(settings, "HEAD", resp.status(), &url));
        }
        Ok(resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()))
    })
}

fn build_http_client(settings: &WebDavSyncSettings) -> Result<Client, AppError> {
    Client::builder()
        .timeout(Duration::from_secs(settings.timeout_secs.max(1)))
        .build()
        .map_err(|e| AppError::Message(format!("创建 WebDAV HTTP 客户端失败: {e}")))
}

fn run_http<F, T>(future: F) -> Result<T, AppError>
where
    F: std::future::Future<Output = Result<T, AppError>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::Message(format!("创建异步运行时失败: {e}")))?;
    runtime.block_on(future)
}

fn zip_skills_ssot(dest_path: &Path) -> Result<(), AppError> {
    let source = SkillService::get_ssot_dir()?;
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let file = fs::File::create(dest_path).map_err(|e| AppError::io(dest_path, e))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip_file_options();

    if source.exists() {
        zip_dir_recursive(&source, &source, &mut writer, options)?;
    }

    writer
        .finish()
        .map_err(|e| AppError::Message(format!("写入 skills.zip 失败: {e}")))?;
    Ok(())
}

fn zip_file_options() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(DateTime::default())
}

/// Zip a CLI config directory, skipping entries whose name matches any exclude pattern.
/// Returns `Ok(None)` if the directory does not exist or is empty.
fn zip_cli_dir(dir: &Path, excludes: &[&str]) -> Result<Option<Vec<u8>>, AppError> {
    if !dir.exists() {
        return Ok(None);
    }

    let tmp = tempdir().map_err(|e| AppError::IoContext {
        context: format!("创建 CLI zip 临时目录失败: {}", dir.display()),
        source: e,
    })?;
    let zip_path = tmp.path().join("cli.zip");

    let file = fs::File::create(&zip_path).map_err(|e| AppError::io(&zip_path, e))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip_file_options();

    zip_dir_recursive_filtered(dir, dir, &mut writer, options, excludes)?;

    writer
        .finish()
        .map_err(|e| AppError::Message(format!("写入 CLI zip 失败: {e}")))?;

    let bytes = fs::read(&zip_path).map_err(|e| AppError::io(&zip_path, e))?;
    // An empty zip with no entries is ~22 bytes; treat as None
    if bytes.len() <= 22 {
        return Ok(None);
    }
    Ok(Some(bytes))
}

fn zip_dir_recursive_filtered(
    root: &Path,
    current: &Path,
    writer: &mut zip::ZipWriter<fs::File>,
    options: SimpleFileOptions,
    excludes: &[&str],
) -> Result<(), AppError> {
    let mut entries = fs::read_dir(current)
        .map_err(|e| AppError::io(current, e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::io(current, e))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Check excludes against the file/dir name
        if excludes.iter().any(|ex| name_str == *ex) {
            continue;
        }

        let rel = path
            .strip_prefix(root)
            .map_err(|e| AppError::Message(format!("生成 ZIP 相对路径失败: {e}")))?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");

        if path.is_dir() {
            writer
                .add_directory(format!("{rel_str}/"), options)
                .map_err(|e| AppError::Message(format!("写入 ZIP 目录失败: {e}")))?;
            zip_dir_recursive_filtered(root, &path, writer, options, excludes)?;
        } else {
            writer
                .start_file(&*rel_str, options)
                .map_err(|e| AppError::Message(format!("写入 ZIP 文件头失败: {e}")))?;
            let mut file = fs::File::open(&path).map_err(|e| AppError::io(&path, e))?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)
                .map_err(|e| AppError::io(&path, e))?;
            writer
                .write_all(&buf)
                .map_err(|e| AppError::Message(format!("写入 ZIP 文件内容失败: {e}")))?;
        }
    }
    Ok(())
}

/// Restore a CLI zip in merge mode: extract into temp dir, then copy over target dir
/// without deleting files that are not in the zip (preserves excluded / local-only files).
fn restore_cli_zip(raw: &[u8], target_dir: &Path) -> Result<(), AppError> {
    let tmp = tempdir().map_err(|e| AppError::IoContext {
        context: "创建 CLI zip 解压临时目录失败".to_string(),
        source: e,
    })?;
    let zip_path = tmp.path().join("cli.zip");
    atomic_write(&zip_path, raw)?;

    let file = fs::File::open(&zip_path).map_err(|e| AppError::io(&zip_path, e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| AppError::Message(format!("解析 CLI zip 失败: {e}")))?;

    let extracted = tmp.path().join("extracted");
    fs::create_dir_all(&extracted).map_err(|e| AppError::io(&extracted, e))?;

    for idx in 0..archive.len() {
        let mut entry = archive
            .by_index(idx)
            .map_err(|e| AppError::Message(format!("读取 ZIP 项失败: {e}")))?;
        let Some(safe_name) = entry.enclosed_name() else {
            continue;
        };
        let out_path = extracted.join(safe_name);
        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| AppError::io(&out_path, e))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
        let mut out = fs::File::create(&out_path).map_err(|e| AppError::io(&out_path, e))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| AppError::io(&out_path, e))?;
    }

    // Merge: copy extracted files over target, creating dirs as needed
    fs::create_dir_all(target_dir).map_err(|e| AppError::io(target_dir, e))?;
    copy_dir_recursive(&extracted, target_dir)?;
    Ok(())
}

fn zip_dir_recursive(
    root: &Path,
    current: &Path,
    writer: &mut zip::ZipWriter<fs::File>,
    options: SimpleFileOptions,
) -> Result<(), AppError> {
    let mut entries = fs::read_dir(current)
        .map_err(|e| AppError::io(current, e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::io(current, e))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map_err(|e| AppError::Message(format!("生成 ZIP 相对路径失败: {e}")))?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");

        if path.is_dir() {
            writer
                .add_directory(format!("{rel_str}/"), options)
                .map_err(|e| AppError::Message(format!("写入 ZIP 目录失败: {e}")))?;
            zip_dir_recursive(root, &path, writer, options)?;
        } else {
            writer
                .start_file(rel_str, options)
                .map_err(|e| AppError::Message(format!("写入 ZIP 文件头失败: {e}")))?;
            let mut file = fs::File::open(&path).map_err(|e| AppError::io(&path, e))?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)
                .map_err(|e| AppError::io(&path, e))?;
            writer
                .write_all(&buf)
                .map_err(|e| AppError::Message(format!("写入 ZIP 文件内容失败: {e}")))?;
        }
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), AppError> {
    if !src.exists() {
        return Ok(());
    }
    fs::create_dir_all(dest).map_err(|e| AppError::io(dest, e))?;
    for entry in fs::read_dir(src).map_err(|e| AppError::io(src, e))? {
        let entry = entry.map_err(|e| AppError::io(src, e))?;
        let path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
            }
            fs::copy(&path, &dest_path).map_err(|e| AppError::io(&dest_path, e))?;
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let hash = hasher.finalize();
    format!("{hash:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::WebDavSyncStatus;

    fn sample_settings() -> WebDavSyncSettings {
        WebDavSyncSettings {
            enabled: true,
            base_url: "https://dav.example.com/remote.php/dav/files/demo/".to_string(),
            remote_root: "cc switch-sync/team a".to_string(),
            profile: "default profile".to_string(),
            username: "demo".to_string(),
            password: "secret".to_string(),
            device_id: "device-a".to_string(),
            timeout_secs: 20,
            status: WebDavSyncStatus::default(),
        }
    }

    #[test]
    fn build_remote_url_encodes_path_segments() {
        let mut settings = sample_settings();
        settings.normalize();
        let rel_segments = remote_file_segments(&settings, REMOTE_MANIFEST);
        let url = build_remote_url(&settings, &rel_segments).expect("build remote url");
        assert_eq!(
            url,
            "https://dav.example.com/remote.php/dav/files/demo/cc%20switch-sync/team%20a/v1/default%20profile/manifest.json"
        );
        assert!(
            !url.contains("//cc"),
            "remote url should not contain duplicated slash before remote path: {url}"
        );
    }

    #[test]
    fn webdav_conflict_error_includes_jianguoyun_hint() {
        let mut settings = sample_settings();
        settings.base_url = "https://dav.jianguoyun.com/dav".to_string();
        let err = webdav_status_error(
            &settings,
            "MKCOL",
            StatusCode::CONFLICT,
            "https://dav.jianguoyun.com/dav/cc-switch-sync",
        );
        assert!(err.to_string().contains("坚果云"));
        assert!(err.to_string().contains("手动创建"));
    }

    #[test]
    fn webdav_not_found_error_includes_jianguoyun_path_hint() {
        let mut settings = sample_settings();
        settings.base_url = "https://dav.jianguoyun.com/dav".to_string();
        let err = webdav_status_error(
            &settings,
            "PROPFIND",
            StatusCode::NOT_FOUND,
            "https://dav.jianguoyun.com/wrong/path",
        );
        assert!(err.to_string().contains("/dav/"));
    }

    #[test]
    fn snapshot_identity_is_stable_when_only_manifest_metadata_changes() {
        let manifest_a = WebDavManifest {
            format: PROTOCOL_FORMAT.to_string(),
            version: PROTOCOL_VERSION,
            updated_at: "2026-02-01T00:00:00Z".to_string(),
            updated_by: "device-a".to_string(),
            artifacts: ManifestArtifacts {
                db_sql: ManifestArtifact {
                    path: REMOTE_DB_SQL.to_string(),
                    sha256: "db-hash".to_string(),
                    size: 1,
                },
                skills_zip: ManifestArtifact {
                    path: REMOTE_SKILLS_ZIP.to_string(),
                    sha256: "skills-hash".to_string(),
                    size: 2,
                },
                settings_sync: ManifestArtifact {
                    path: REMOTE_SETTINGS_SYNC.to_string(),
                    sha256: "settings-hash".to_string(),
                    size: 3,
                },
                claude_zip: None,
                codex_zip: None,
                gemini_zip: None,
                memory_sql: None,
            },
        };
        let manifest_b = WebDavManifest {
            updated_at: "2026-02-02T00:00:00Z".to_string(),
            updated_by: "device-b".to_string(),
            ..manifest_a.clone()
        };
        assert_eq!(
            snapshot_identity_from_manifest(&manifest_a),
            snapshot_identity_from_manifest(&manifest_b)
        );
    }

    #[test]
    fn syncable_settings_json_is_stable_for_endpoint_maps() {
        let endpoint_a = CustomEndpoint {
            url: "https://a.example.com".to_string(),
            added_at: 1,
            last_used: None,
        };
        let endpoint_b = CustomEndpoint {
            url: "https://b.example.com".to_string(),
            added_at: 2,
            last_used: Some(3),
        };

        let mut map_first = BTreeMap::new();
        map_first.insert("b".to_string(), endpoint_b.clone());
        map_first.insert("a".to_string(), endpoint_a.clone());

        let mut map_second = BTreeMap::new();
        map_second.insert("a".to_string(), endpoint_a);
        map_second.insert("b".to_string(), endpoint_b);

        let settings_first = SyncableSettings {
            language: Some("en".to_string()),
            skill_sync_method: crate::services::skill::SyncMethod::Auto,
            security: None,
            custom_endpoints_claude: map_first.clone(),
            custom_endpoints_codex: map_first,
        };
        let settings_second = SyncableSettings {
            language: Some("en".to_string()),
            skill_sync_method: crate::services::skill::SyncMethod::Auto,
            security: None,
            custom_endpoints_claude: map_second.clone(),
            custom_endpoints_codex: map_second,
        };

        let first = serde_json::to_string_pretty(&settings_first).expect("serialize settings #1");
        let second = serde_json::to_string_pretty(&settings_second).expect("serialize settings #2");
        assert_eq!(first, second);
    }

    #[test]
    fn mkcol_405_and_409_require_post_verification() {
        assert!(should_verify_after_mkcol(StatusCode::METHOD_NOT_ALLOWED));
        assert!(should_verify_after_mkcol(StatusCode::CONFLICT));
        assert!(should_verify_after_mkcol(StatusCode::TEMPORARY_REDIRECT));
        assert!(should_verify_after_mkcol(StatusCode::PERMANENT_REDIRECT));
        assert!(!should_verify_after_mkcol(StatusCode::CREATED));
    }

    #[test]
    fn zip_output_is_stable_for_same_content() {
        let tmp = tempdir().expect("create temp dir");
        let source = tmp.path().join("skills");
        fs::create_dir_all(source.join("nested")).expect("create source dirs");
        fs::write(source.join("b.txt"), b"bbb").expect("write b");
        fs::write(source.join("nested").join("a.txt"), b"aaa").expect("write a");

        let zip1 = tmp.path().join("first.zip");
        let zip2 = tmp.path().join("second.zip");

        let file1 = fs::File::create(&zip1).expect("create zip1");
        let mut writer1 = zip::ZipWriter::new(file1);
        zip_dir_recursive(&source, &source, &mut writer1, zip_file_options())
            .expect("zip source #1");
        writer1.finish().expect("finish zip1");

        std::thread::sleep(std::time::Duration::from_secs(1));

        let file2 = fs::File::create(&zip2).expect("create zip2");
        let mut writer2 = zip::ZipWriter::new(file2);
        zip_dir_recursive(&source, &source, &mut writer2, zip_file_options())
            .expect("zip source #2");
        writer2.finish().expect("finish zip2");

        let bytes1 = fs::read(&zip1).expect("read zip1");
        let bytes2 = fs::read(&zip2).expect("read zip2");
        assert_eq!(bytes1, bytes2, "zip output should be deterministic");
    }
}
