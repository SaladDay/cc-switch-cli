//! Transport-independent cloud-sync protocol.
//!
//! WebDAV and S3 deliberately share the same manifest and artifact format so
//! either transport receives the same validation and restore guarantees.

use std::collections::BTreeMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

use crate::database::{Database, SCHEMA_VERSION};
use crate::error::AppError;

use super::webdav_sync::archive::{restore_skills_zip, zip_skills_ssot, SkillsBackup};

pub(crate) const PROTOCOL_FORMAT: &str = "cc-switch-webdav-sync";
pub(crate) const PROTOCOL_VERSION: u32 = 2;
pub(crate) const DB_COMPAT_VERSION: u32 = 6;
pub(crate) const LEGACY_DB_COMPAT_VERSION: u32 = 5;
pub(crate) const REMOTE_DB_SQL: &str = "db.sql";
pub(crate) const REMOTE_SKILLS_ZIP: &str = "skills.zip";
pub(crate) const REMOTE_MANIFEST: &str = "manifest.json";
pub(crate) const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_SYNC_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) const MAX_DEVICE_NAME_LEN: usize = 64;

pub(crate) fn localized(
    key: &'static str,
    zh: impl Into<String>,
    en: impl Into<String>,
) -> AppError {
    AppError::localized(key, zh, en)
}

fn io_context_localized(
    _key: &'static str,
    zh: impl Into<String>,
    en: impl Into<String>,
    source: std::io::Error,
) -> AppError {
    let zh_message = zh.into();
    let en_message = en.into();
    AppError::IoContext {
        context: format!("{zh_message} ({en_message})"),
        source,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncManifest {
    pub format: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_compat_version: Option<u32>,
    pub device_name: String,
    pub created_at: String,
    pub artifacts: BTreeMap<String, ArtifactMeta>,
    pub snapshot_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ArtifactMeta {
    pub sha256: String,
    pub size: u64,
}

pub(crate) struct LocalSnapshot {
    pub db_sql: Vec<u8>,
    pub skills_zip: Vec<u8>,
    pub manifest_bytes: Vec<u8>,
    pub manifest_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteLayout {
    Current,
    Legacy,
}

impl RemoteLayout {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Legacy => "legacy",
        }
    }
}

pub(crate) fn build_local_snapshot() -> Result<LocalSnapshot, AppError> {
    let db_sql = Database::init()?.export_sql_string_for_sync()?.into_bytes();

    let temp = tempdir().map_err(|error| {
        io_context_localized(
            "sync.snapshot_tmpdir_failed",
            "创建同步快照临时目录失败",
            "Failed to create temporary directory for sync snapshot",
            error,
        )
    })?;
    let skills_zip_path = temp.path().join(REMOTE_SKILLS_ZIP);
    zip_skills_ssot(&skills_zip_path)?;
    let skills_zip =
        std::fs::read(&skills_zip_path).map_err(|error| AppError::io(&skills_zip_path, error))?;

    let mut artifacts = BTreeMap::new();
    artifacts.insert(
        REMOTE_DB_SQL.to_string(),
        ArtifactMeta {
            sha256: sha256_hex(&db_sql),
            size: db_sql.len() as u64,
        },
    );
    artifacts.insert(
        REMOTE_SKILLS_ZIP.to_string(),
        ArtifactMeta {
            sha256: sha256_hex(&skills_zip),
            size: skills_zip.len() as u64,
        },
    );

    let manifest = SyncManifest {
        format: PROTOCOL_FORMAT.to_string(),
        version: PROTOCOL_VERSION,
        db_compat_version: Some(DB_COMPAT_VERSION),
        device_name: detect_system_device_name().unwrap_or_else(|| "Unknown Device".to_string()),
        created_at: Utc::now().to_rfc3339(),
        snapshot_id: compute_snapshot_id(&artifacts),
        artifacts,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|source| AppError::JsonSerialize { source })?;
    let manifest_hash = sha256_hex(&manifest_bytes);

    Ok(LocalSnapshot {
        db_sql,
        skills_zip,
        manifest_bytes,
        manifest_hash,
    })
}

pub(crate) fn effective_db_compat_version(
    manifest: &SyncManifest,
    layout: RemoteLayout,
) -> Option<u32> {
    manifest
        .db_compat_version
        .or_else(|| (layout == RemoteLayout::Legacy).then_some(LEGACY_DB_COMPAT_VERSION))
}

pub(crate) fn validate_manifest_compat(
    manifest: &SyncManifest,
    layout: RemoteLayout,
) -> Result<(), AppError> {
    if manifest.format != PROTOCOL_FORMAT {
        return Err(localized(
            "sync.manifest_format_incompatible",
            format!("远端 manifest 格式不兼容: {}", manifest.format),
            format!(
                "Remote manifest format is incompatible: {}",
                manifest.format
            ),
        ));
    }
    if manifest.version != PROTOCOL_VERSION {
        return Err(localized(
            "sync.manifest_version_incompatible",
            format!(
                "远端 manifest 协议版本不兼容: v{} (本地 v{PROTOCOL_VERSION})",
                manifest.version
            ),
            format!(
                "Remote manifest protocol version is incompatible: v{} (local v{PROTOCOL_VERSION})",
                manifest.version
            ),
        ));
    }

    let Some(db_compat_version) = effective_db_compat_version(manifest, layout) else {
        return Err(localized(
            "sync.manifest_db_version_missing",
            "远端 manifest 缺少数据库兼容版本",
            "Remote manifest is missing the database compatibility version.",
        ));
    };

    match layout {
        RemoteLayout::Current if db_compat_version != DB_COMPAT_VERSION => {
            return Err(localized(
                "sync.manifest_db_version_incompatible",
                format!(
                    "远端数据库快照版本不兼容: db-v{db_compat_version} (本地 db-v{DB_COMPAT_VERSION})"
                ),
                format!(
                    "Remote database snapshot version is incompatible: db-v{db_compat_version} (local db-v{DB_COMPAT_VERSION})"
                ),
            ));
        }
        RemoteLayout::Legacy if db_compat_version > DB_COMPAT_VERSION => {
            return Err(localized(
                "sync.manifest_db_version_incompatible",
                format!(
                    "远端数据库快照版本不兼容: db-v{db_compat_version} (本地最高支持 db-v{DB_COMPAT_VERSION})"
                ),
                format!(
                    "Remote database snapshot version is incompatible: db-v{db_compat_version} (local supports up to db-v{DB_COMPAT_VERSION})"
                ),
            ));
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn validate_artifact_size_limit(name: &str, size: u64) -> Result<(), AppError> {
    if size > MAX_SYNC_ARTIFACT_BYTES {
        let max_mb = MAX_SYNC_ARTIFACT_BYTES / 1024 / 1024;
        return Err(localized(
            "sync.artifact_too_large",
            format!("artifact {name} 超过下载上限（{max_mb} MB）"),
            format!("Artifact {name} exceeds download limit ({max_mb} MB)"),
        ));
    }
    Ok(())
}

pub(crate) fn verify_artifact(
    bytes: &[u8],
    artifact_name: &str,
    meta: &ArtifactMeta,
) -> Result<(), AppError> {
    if bytes.len() as u64 != meta.size {
        return Err(localized(
            "sync.artifact_size_mismatch",
            format!(
                "artifact {artifact_name} 大小不匹配 (expected: {}, got: {})",
                meta.size,
                bytes.len()
            ),
            format!(
                "Artifact {artifact_name} size mismatch (expected: {}, got: {})",
                meta.size,
                bytes.len()
            ),
        ));
    }

    let actual_hash = sha256_hex(bytes);
    if actual_hash != meta.sha256 {
        return Err(localized(
            "sync.artifact_hash_mismatch",
            format!(
                "artifact {artifact_name} SHA256 校验失败 (expected: {}..., got: {}...)",
                meta.sha256.get(..8).unwrap_or(&meta.sha256),
                actual_hash.get(..8).unwrap_or(&actual_hash)
            ),
            format!(
                "Artifact {artifact_name} SHA256 verification failed (expected: {}..., got: {}...)",
                meta.sha256.get(..8).unwrap_or(&meta.sha256),
                actual_hash.get(..8).unwrap_or(&actual_hash)
            ),
        ));
    }
    Ok(())
}

/// Apply a verified snapshot while preserving the local WebDAV implementation's
/// future-schema preflight and Skills rollback guarantees.
pub(crate) fn apply_snapshot(db_sql: &[u8], skills_zip: &[u8]) -> Result<(), AppError> {
    let sql = std::str::from_utf8(db_sql).map_err(|error| {
        localized(
            "sync.sql_not_utf8",
            format!("SQL 非 UTF-8: {error}"),
            format!("SQL is not valid UTF-8: {error}"),
        )
    })?;
    validate_sql_user_version_for_import(sql)?;

    let skills_backup = SkillsBackup::backup_current_skills()?;
    restore_skills_zip(skills_zip)?;

    if let Err(db_error) = Database::init().and_then(|db| db.import_sql_string_for_sync(sql)) {
        if let Err(rollback_error) = skills_backup.restore() {
            return Err(localized(
                "sync.db_import_and_rollback_failed",
                format!("导入数据库失败: {db_error}; 同时回滚 Skills 失败: {rollback_error}"),
                format!(
                    "Database import failed: {db_error}; skills rollback also failed: {rollback_error}"
                ),
            ));
        }
        return Err(db_error);
    }

    Ok(())
}

pub(crate) async fn apply_snapshot_with_restore_guard(
    db_sql: &[u8],
    skills_zip: &[u8],
) -> Result<(), AppError> {
    let _guard = super::state_coordination::acquire_restore_mutation_guard()
        .await
        .map_err(AppError::Message)?;
    ensure_restore_allowed().await?;
    apply_snapshot(db_sql, skills_zip)
}

async fn ensure_restore_allowed() -> Result<(), AppError> {
    let db = std::sync::Arc::new(Database::init()?);
    let proxy_service = super::ProxyService::new(db);
    if proxy_service.get_status().await.running {
        return Err(localized(
            "sync.restore_proxy_running",
            "本地代理正在运行，请先停止代理后再执行云同步恢复",
            "The local proxy is running. Stop it before restoring a cloud-sync snapshot.",
        ));
    }

    let takeover_active = proxy_service
        .is_app_takeover_active(&crate::AppType::Claude)
        .await
        .map_err(AppError::Message)?
        || proxy_service
            .is_app_takeover_active(&crate::AppType::Codex)
            .await
            .map_err(AppError::Message)?
        || proxy_service
            .is_app_takeover_active(&crate::AppType::Gemini)
            .await
            .map_err(AppError::Message)?;
    if takeover_active {
        return Err(localized(
            "sync.restore_takeover_active",
            "当前仍有应用处于代理接管状态，请先关闭接管后再执行云同步恢复",
            "An app takeover is still active. Disable takeover before restoring a cloud-sync snapshot.",
        ));
    }
    Ok(())
}

pub(crate) fn validate_sql_user_version_for_import(sql: &str) -> Result<(), AppError> {
    let Some(version) = extract_sql_user_version(sql) else {
        return Ok(());
    };
    if version > SCHEMA_VERSION {
        return Err(localized(
            "sync.db_schema_too_new",
            format!(
                "远端数据库版本过新（{version}），当前应用仅支持 {SCHEMA_VERSION}，请先升级应用后再同步"
            ),
            format!(
                "Remote database schema is too new ({version}); this app supports up to {SCHEMA_VERSION}. Upgrade before syncing."
            ),
        ));
    }
    Ok(())
}

pub(crate) fn extract_sql_user_version(sql: &str) -> Option<i32> {
    sql.lines().find_map(|line| {
        let trimmed = line.trim_start_matches('\u{feff}').trim();
        let value = trimmed
            .strip_prefix("PRAGMA user_version")
            .and_then(|rest| rest.trim_start().strip_prefix('='))
            .map(|rest| rest.trim().trim_end_matches(';').trim())
            .or_else(|| trimmed.strip_prefix("-- user_version:").map(str::trim))?;
        value.parse::<i32>().ok()
    })
}

pub(crate) fn compute_snapshot_id(artifacts: &BTreeMap<String, ArtifactMeta>) -> String {
    let combined = artifacts
        .iter()
        .map(|(name, meta)| format!("{name}:{}", meta.sha256))
        .collect::<Vec<_>>()
        .join("|");
    sha256_hex(combined.as_bytes())
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub(crate) fn detect_system_device_name() -> Option<String> {
    let env_name = ["CC_SWITCH_DEVICE_NAME", "COMPUTERNAME", "HOSTNAME"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find_map(|value| normalize_device_name(&value));
    if env_name.is_some() {
        return env_name;
    }

    let output = std::process::Command::new("hostname").output().ok()?;
    if !output.status.success() {
        return None;
    }
    normalize_device_name(&String::from_utf8(output.stdout).ok()?)
}

pub(crate) fn normalize_device_name(raw: &str) -> Option<String> {
    let compact = raw
        .chars()
        .fold(String::with_capacity(raw.len()), |mut result, character| {
            if character.is_whitespace() {
                result.push(' ');
            } else if !character.is_control() {
                result.push(character);
            }
            result
        });
    let normalized = compact.split_whitespace().collect::<Vec<_>>().join(" ");
    let limited = normalized
        .trim()
        .chars()
        .take(MAX_DEVICE_NAME_LEN)
        .collect::<String>();
    (!limited.is_empty()).then_some(limited)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(db_compat_version: Option<u32>) -> SyncManifest {
        SyncManifest {
            format: PROTOCOL_FORMAT.to_string(),
            version: PROTOCOL_VERSION,
            db_compat_version,
            device_name: "test-device".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            artifacts: BTreeMap::new(),
            snapshot_id: "snapshot".to_string(),
        }
    }

    #[test]
    fn future_database_schema_is_rejected_before_restore() {
        let sql = format!("PRAGMA user_version={};\n", SCHEMA_VERSION + 1);
        assert!(validate_sql_user_version_for_import(&sql).is_err());
    }

    #[test]
    fn current_database_schema_is_accepted() {
        let sql = format!("PRAGMA user_version={SCHEMA_VERSION};\n");
        assert!(validate_sql_user_version_for_import(&sql).is_ok());
    }

    #[test]
    fn custom_endpoint_behavior_remains_a_transport_concern() {
        assert_eq!(PROTOCOL_FORMAT, "cc-switch-webdav-sync");
        assert_eq!(PROTOCOL_VERSION, 2);
        assert_eq!(DB_COMPAT_VERSION, 6);
    }

    #[test]
    fn normalize_device_name_is_bounded_and_human_readable() {
        assert_eq!(
            normalize_device_name("  Mac\tBook \n Pro\u{0007} "),
            Some("Mac Book Pro".to_string())
        );
        assert_eq!(normalize_device_name(&"a".repeat(80)).unwrap().len(), 64);
    }

    #[test]
    fn current_layout_requires_exact_database_compatibility() {
        assert!(validate_manifest_compat(
            &manifest(Some(DB_COMPAT_VERSION)),
            RemoteLayout::Current
        )
        .is_ok());
        assert!(validate_manifest_compat(
            &manifest(Some(DB_COMPAT_VERSION + 1)),
            RemoteLayout::Current
        )
        .is_err());
        assert!(validate_manifest_compat(
            &manifest(Some(DB_COMPAT_VERSION - 1)),
            RemoteLayout::Current
        )
        .is_err());
    }

    #[test]
    fn artifact_verification_checks_size_and_hash() {
        let bytes = b"snapshot";
        let metadata = ArtifactMeta {
            sha256: sha256_hex(bytes),
            size: bytes.len() as u64,
        };
        assert!(verify_artifact(bytes, "db.sql", &metadata).is_ok());
        assert!(verify_artifact(b"changed", "db.sql", &metadata).is_err());
    }
}
