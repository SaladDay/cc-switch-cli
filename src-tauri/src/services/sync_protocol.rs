//! Transport-independent cloud-sync protocol.
//!
//! WebDAV and S3 deliberately share the same manifest and artifact format so
//! either transport receives the same validation and restore guarantees.

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

use crate::database::{Database, SCHEMA_VERSION};
use crate::error::AppError;
use crate::skill_directory::SkillDirectory;
use crate::store::AppState;

use super::skills_restore::{require_exact_skill_payload, PreparedSkillsRestore};
use super::webdav_sync::archive::zip_skills_ssot;
use super::{RestoreCoordinator, RestorePublication};

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

#[derive(Debug)]
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
    // SQLite rows and the Skills tree form one logical snapshot. Ordinary
    // workflows may update them in separate durable steps, so a shared permit
    // cannot make the cross-resource read coherent.
    let snapshot_permit =
        super::state_coordination::acquire_consistent_state_snapshot_permit_blocking()
            .map_err(AppError::Message)?;
    let database = Database::init()?;
    let expected_skill_directories = database_skill_directories(&database)?;
    let db_sql = database.export_sql_string_for_sync()?.into_bytes();

    let temp = tempdir().map_err(|error| {
        io_context_localized(
            "sync.snapshot_tmpdir_failed",
            "创建同步快照临时目录失败",
            "Failed to create temporary directory for sync snapshot",
            error,
        )
    })?;
    let skills_zip_path = temp.path().join(REMOTE_SKILLS_ZIP);
    let payload_skill_directories = zip_skills_ssot(&skills_zip_path)?;
    require_exact_skill_payload(&expected_skill_directories, &payload_skill_directories)?;
    let skills_zip =
        std::fs::read(&skills_zip_path).map_err(|error| AppError::io(&skills_zip_path, error))?;
    drop(snapshot_permit);

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

fn database_skill_directories(database: &Database) -> Result<BTreeSet<String>, AppError> {
    let mut identities = BTreeMap::<String, String>::new();
    let mut directories = BTreeSet::new();
    for skill in database.get_all_installed_skills()?.into_values() {
        let directory = SkillDirectory::parse(&skill.directory).map_err(|error| {
            AppError::InvalidInput(format!(
                "invalid Skill directory {:?} in local database: {error}",
                skill.directory
            ))
        })?;
        let collision_key = directory.collision_key();
        if let Some(existing) = identities.insert(collision_key, directory.as_str().to_string()) {
            return Err(AppError::InvalidInput(format!(
                "local Skills database contains duplicate or normalization-colliding directories {existing:?} and {:?}",
                directory.as_str()
            )));
        }
        directories.insert(directory.as_str().to_string());
    }
    Ok(directories)
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
    let limit = sync_artifact_size_limit(name);
    if size > limit {
        let max_mb = limit / 1024 / 1024;
        return Err(localized(
            "sync.artifact_too_large",
            format!("artifact {name} 超过下载上限（{max_mb} MB）"),
            format!("Artifact {name} exceeds download limit ({max_mb} MB)"),
        ));
    }
    Ok(())
}

pub(crate) fn sync_artifact_size_limit(name: &str) -> u64 {
    if name == REMOTE_DB_SQL {
        crate::database::MAX_SQL_IMPORT_BYTES
    } else {
        MAX_SYNC_ARTIFACT_BYTES
    }
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
#[cfg(test)]
pub(crate) fn apply_snapshot(db_sql: &[u8], skills_zip: &[u8]) -> Result<(), AppError> {
    let restore = RestoreCoordinator::acquire_blocking()?;
    let state = restore.load_state()?;
    let (prepared_database, prepared_skills) =
        prepare_snapshot(restore.operation(), db_sql, skills_zip)?;
    restore
        .publish(&state, prepared_database, Some(prepared_skills), |_| ())
        .map(|_| ())
}

fn prepare_snapshot(
    operation: crate::restore_protocol::RestoreOperationId,
    db_sql: &[u8],
    skills_zip: &[u8],
) -> Result<
    (
        crate::database::PreparedDatabaseRestore,
        PreparedSkillsRestore,
    ),
    AppError,
> {
    let sql = std::str::from_utf8(db_sql).map_err(|error| {
        localized(
            "sync.sql_not_utf8",
            format!("SQL 非 UTF-8: {error}"),
            format!("SQL is not valid UTF-8: {error}"),
        )
    })?;
    validate_sql_user_version_for_import(sql)?;

    let prepared_database = Database::prepare_sql_string_for_sync(sql)?;
    let expected_directories = prepared_database.skill_directories()?;
    let prepared_skills =
        PreparedSkillsRestore::prepare(operation, skills_zip, &expected_directories)?;
    Ok((prepared_database, prepared_skills))
}

pub(crate) async fn apply_snapshot_with_restore_guard_and_then<Snapshot>(
    db_sql: &[u8],
    skills_zip: &[u8],
    snapshot: impl FnOnce(&AppState) -> Snapshot,
) -> Result<RestorePublication<Snapshot>, AppError> {
    let restore = RestoreCoordinator::acquire().await?;
    let state = restore.load_state()?;
    let (prepared_database, prepared_skills) =
        prepare_snapshot(restore.operation(), db_sql, skills_zip)?;
    let completion = restore.publish(&state, prepared_database, Some(prepared_skills), snapshot)?;
    Ok(RestorePublication {
        token: completion.publication,
        status: completion.status,
        snapshot: completion.snapshot,
    })
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

    fn installed_skill(directory: &str) -> crate::app_config::InstalledSkill {
        crate::app_config::InstalledSkill {
            id: format!("local:{directory}"),
            name: directory.to_string(),
            description: None,
            directory: directory.to_string(),
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            readme_url: None,
            apps: crate::app_config::SkillApps::default(),
            installed_at: 0,
        }
    }

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
    #[serial_test::serial(home_settings)]
    fn snapshot_rejects_skill_database_rows_without_file_payload() {
        let home = tempfile::tempdir().expect("create isolated sync home");
        let _environment = crate::test_support::TestEnvGuard::isolated(home.path());
        let database = Database::init().expect("initialize database");
        database
            .save_skill(&installed_skill("missing-payload"))
            .expect("save Skill metadata");
        drop(database);

        let error =
            build_local_snapshot().expect_err("metadata without files must reject the snapshot");
        assert!(error.to_string().contains("database/files mismatch"));
    }

    #[test]
    #[serial_test::serial(home_settings)]
    fn snapshot_rejects_skill_file_payload_without_database_row() {
        let home = tempfile::tempdir().expect("create isolated sync home");
        let _environment = crate::test_support::TestEnvGuard::isolated(home.path());
        drop(Database::init().expect("initialize database"));
        let skill = crate::services::SkillService::get_ssot_dir()
            .expect("resolve Skills root")
            .join("unexpected-payload");
        std::fs::create_dir_all(&skill).expect("create unexpected Skill");
        std::fs::write(skill.join("SKILL.md"), b"unexpected").expect("write Skill payload");

        let error =
            build_local_snapshot().expect_err("files without metadata must reject the snapshot");
        assert!(error.to_string().contains("database/files mismatch"));
    }

    #[test]
    fn future_database_schema_is_rejected_before_restore() {
        let sql = format!("PRAGMA user_version={};\n", SCHEMA_VERSION + 1);
        assert!(validate_sql_user_version_for_import(&sql).is_err());
    }

    #[test]
    fn database_artifact_uses_the_sql_import_limit_before_download() {
        let sql_limit = 256 * 1024 * 1024;
        assert!(
            validate_artifact_size_limit(REMOTE_DB_SQL, sql_limit + 1).is_err(),
            "db.sql must be rejected at its importer limit, before transport allocation"
        );
        assert!(
            validate_artifact_size_limit(REMOTE_SKILLS_ZIP, sql_limit + 1).is_ok(),
            "skills archives keep their independent transport limit"
        );
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
