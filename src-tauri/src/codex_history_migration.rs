//! Codex 第三方历史会话归桶迁移。
//!
//! 只迁移本机 `~/.codex` 历史数据；完成标记写入设备级 `settings.json`，
//! 失败时不写标记，下一次启动自动重试。

use crate::codex_config::{
    get_codex_config_dir, read_codex_config_text, CC_SWITCH_CODEX_MODEL_PROVIDER_ID,
};
use crate::codex_state_db::codex_state_db_paths;
#[cfg(test)]
use crate::codex_state_db::CODEX_STATE_DB_FILENAME;
use crate::config::{
    atomic_write, copy_file, create_managed_config_parent_dirs, get_app_config_dir,
};
use crate::database::{is_official_seed_id, run_sqlite_backup_to_completion, Database};
use crate::error::AppError;
use crate::settings::{
    CodexOfficialHistoryUnifyMigration, CodexProviderTemplateMigration,
    CodexThirdPartyHistoryProviderBucketMigration,
};
use chrono::{Local, Utc};
use rusqlite::{backup::Backup, params_from_iter, Connection, OpenFlags};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use toml_edit::DocumentMut;

const MIGRATION_NAME: &str = "codex-history-provider-migration-v2";
const OFFICIAL_UNIFY_MIGRATION_NAME: &str = "codex-official-history-unify-v1";
/// 还原操作自身的备份目录（与迁移备份分开，保持迁移账本目录纯净）。
const OFFICIAL_UNIFY_RESTORE_BACKUP_NAME: &str = "codex-official-history-unify-restore-v1";
/// SQLite 变量上限保守值，IN 列表按此分块。
const STATE_DB_ID_CHUNK: usize = 500;

/// 串行化官方历史的迁移与还原：开启迁移和关闭还原可能先后触发，
/// 需要避免对同一批 jsonl / state DB 双向并发改写。
static CODEX_OFFICIAL_HISTORY_OP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_codex_official_history_op() -> std::sync::MutexGuard<'static, ()> {
    CODEX_OFFICIAL_HISTORY_OP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Codex 内建默认 provider id：config.toml 没有 `model_provider` 键时会话归入此桶。
const OFFICIAL_OPENAI_CODEX_MODEL_PROVIDER_ID: &str = "openai";
const LEGACY_CC_SWITCH_CODEX_MODEL_PROVIDER_ID: &str = "ccswitch";
// If a Codex preset ever used a temporary routing key, keep that old key here
// so local history can be bucketed under the current custom provider id.
const CC_SWITCH_LEGACY_CODEX_MODEL_PROVIDER_IDS: &[&str] = &[
    LEGACY_CC_SWITCH_CODEX_MODEL_PROVIDER_ID,
    "aicodemirror",
    "aicoding",
    "aigocode",
    "aihubmix",
    "ark_agentplan",
    "bailian",
    "bailing",
    "byteplus",
    "claudecn",
    "compshare",
    "compshare_coding",
    "crazyrouter",
    "ctok",
    "cubence",
    "deepseek",
    "dmxapi",
    "doubaoseed",
    "eflowcode",
    "etok",
    "kimi",
    "lemondata",
    "longcat",
    "micu",
    "minimax",
    "minimax_en",
    "modelscope",
    "novita",
    "nvidia",
    "openrouter",
    "packycode",
    "patewayai",
    "pipellm",
    "qianfan_coding",
    "relaxycode",
    "rightcode",
    "runapi",
    "shengsuanyun",
    "siliconflow",
    "siliconflow_en",
    "sssaicode",
    "stepfun",
    "stepfun_en",
    "therouter",
    "xiaomi_mimo",
    "xiaomi_mimo_token_plan",
    "zhipu_glm",
    "zhipu_glm_en",
];

#[derive(Debug, Clone, Default)]
pub struct CodexHistoryProviderBucketMigrationOutcome {
    pub source_provider_ids: Vec<String>,
    pub migrated_jsonl_files: usize,
    pub migrated_state_rows: usize,
    pub skipped_reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CodexProviderTemplateBucketMigrationOutcome {
    pub migrated_provider_ids: Vec<String>,
    pub skipped_reason: Option<String>,
}

pub fn maybe_migrate_codex_third_party_history_provider_bucket(
    db: &Database,
) -> Result<CodexHistoryProviderBucketMigrationOutcome, AppError> {
    if crate::settings::is_codex_third_party_history_provider_bucket_migrated() {
        return Ok(CodexHistoryProviderBucketMigrationOutcome {
            skipped_reason: Some("already_migrated".to_string()),
            ..Default::default()
        });
    }

    let source_provider_ids = collect_source_model_provider_ids(db)?;
    if source_provider_ids.is_empty() {
        crate::settings::mark_codex_third_party_history_provider_bucket_migrated(
            CodexThirdPartyHistoryProviderBucketMigration {
                completed_at: Utc::now().to_rfc3339(),
                target_provider_id: CC_SWITCH_CODEX_MODEL_PROVIDER_ID.to_string(),
                source_provider_ids: Vec::new(),
                migrated_jsonl_files: 0,
                migrated_state_rows: 0,
                scanned_history_files: true,
            },
        )?;
        return Ok(CodexHistoryProviderBucketMigrationOutcome {
            skipped_reason: Some("no_third_party_provider_ids".to_string()),
            ..Default::default()
        });
    }

    let backup_root = migration_backup_root(MIGRATION_NAME);
    let codex_dir = get_codex_config_dir();
    let migrated_jsonl_files =
        migrate_codex_jsonl_files(&codex_dir, &source_provider_ids, &backup_root)?;
    let migrated_state_rows =
        migrate_codex_state_dbs(&codex_dir, &source_provider_ids, &backup_root)?;

    let source_provider_ids_vec: Vec<String> = source_provider_ids.iter().cloned().collect();
    crate::settings::mark_codex_third_party_history_provider_bucket_migrated(
        CodexThirdPartyHistoryProviderBucketMigration {
            completed_at: Utc::now().to_rfc3339(),
            target_provider_id: CC_SWITCH_CODEX_MODEL_PROVIDER_ID.to_string(),
            source_provider_ids: source_provider_ids_vec.clone(),
            migrated_jsonl_files,
            migrated_state_rows,
            scanned_history_files: true,
        },
    )?;

    Ok(CodexHistoryProviderBucketMigrationOutcome {
        source_provider_ids: source_provider_ids_vec,
        migrated_jsonl_files,
        migrated_state_rows,
        skipped_reason: None,
    })
}

pub fn maybe_migrate_codex_provider_template_bucket(
    db: &Database,
) -> Result<CodexProviderTemplateBucketMigrationOutcome, AppError> {
    if crate::settings::is_codex_provider_template_migrated() {
        return Ok(CodexProviderTemplateBucketMigrationOutcome {
            skipped_reason: Some("already_migrated".to_string()),
            ..Default::default()
        });
    }

    let backup_root = migration_backup_root(MIGRATION_NAME);
    let outcome = migrate_codex_provider_templates_to_custom(db, &backup_root)?;
    crate::settings::mark_codex_provider_template_migrated(CodexProviderTemplateMigration {
        completed_at: Utc::now().to_rfc3339(),
        migrated_provider_ids: outcome.migrated_provider_ids.clone(),
    })?;

    Ok(outcome)
}

/// 统一会话开关的存量迁移：把官方会话（内建 "openai" 桶）迁入共享 "custom" 桶。
pub fn maybe_migrate_codex_official_history_to_unified_bucket(
) -> Result<CodexHistoryProviderBucketMigrationOutcome, AppError> {
    if !crate::settings::unify_codex_session_history() {
        return Ok(CodexHistoryProviderBucketMigrationOutcome {
            skipped_reason: Some("unify_toggle_off".to_string()),
            ..Default::default()
        });
    }
    if !crate::settings::unify_codex_migrate_existing_requested() {
        return Ok(CodexHistoryProviderBucketMigrationOutcome {
            skipped_reason: Some("stock_migration_not_requested".to_string()),
            ..Default::default()
        });
    }

    let _op_guard = lock_codex_official_history_op();
    let codex_dir = get_codex_config_dir();
    let codex_dir_key = canonical_dir_string(&codex_dir);
    if crate::settings::is_codex_official_history_unify_migrated_for_dir(&codex_dir_key) {
        return Ok(CodexHistoryProviderBucketMigrationOutcome {
            skipped_reason: Some("already_migrated".to_string()),
            ..Default::default()
        });
    }

    if !codex_config_text_routes_custom(&read_codex_config_text().unwrap_or_default()) {
        return Ok(CodexHistoryProviderBucketMigrationOutcome {
            skipped_reason: Some("live_not_unified".to_string()),
            ..Default::default()
        });
    }

    let source_provider_ids: BTreeSet<String> =
        std::iter::once(OFFICIAL_OPENAI_CODEX_MODEL_PROVIDER_ID.to_string()).collect();
    let backup_root = migration_backup_root(OFFICIAL_UNIFY_MIGRATION_NAME);
    let migrated_jsonl_files =
        migrate_codex_jsonl_files(&codex_dir, &source_provider_ids, &backup_root)?;
    let migrated_state_rows =
        migrate_codex_state_dbs(&codex_dir, &source_provider_ids, &backup_root)?;
    write_backup_generation_meta(&backup_root, &codex_dir_key)?;

    let outcome = CodexHistoryProviderBucketMigrationOutcome {
        source_provider_ids: source_provider_ids.into_iter().collect(),
        migrated_jsonl_files,
        migrated_state_rows,
        skipped_reason: None,
    };

    let marker_written = crate::settings::mark_codex_official_history_unify_migrated_if_enabled(
        CodexOfficialHistoryUnifyMigration {
            completed_at: Utc::now().to_rfc3339(),
            target_provider_id: CC_SWITCH_CODEX_MODEL_PROVIDER_ID.to_string(),
            migrated_jsonl_files,
            migrated_state_rows,
            codex_config_dir: Some(codex_dir_key),
        },
    )?;
    if !marker_written {
        return Ok(CodexHistoryProviderBucketMigrationOutcome {
            skipped_reason: Some("toggle_disabled_during_migration".to_string()),
            ..outcome
        });
    }

    Ok(outcome)
}

fn codex_config_text_routes_custom(config_text: &str) -> bool {
    config_text
        .parse::<DocumentMut>()
        .ok()
        .and_then(|doc| {
            doc.get("model_provider")
                .and_then(|item| item.as_str())
                .map(|id| id.trim() == CC_SWITCH_CODEX_MODEL_PROVIDER_ID)
        })
        .unwrap_or(false)
}

fn canonical_dir_string(dir: &Path) -> String {
    fs::canonicalize(dir)
        .unwrap_or_else(|_| dir.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn write_backup_generation_meta(backup_root: &Path, codex_dir_key: &str) -> Result<(), AppError> {
    if !backup_root.exists() {
        return Ok(());
    }
    let payload = serde_json::json!({ "codexConfigDir": codex_dir_key });
    let bytes =
        serde_json::to_vec_pretty(&payload).map_err(|e| AppError::JsonSerialize { source: e })?;
    atomic_write(&backup_root.join("meta.json"), &bytes)
}

#[derive(Debug, Clone, Default)]
pub struct CodexOfficialHistoryRestoreOutcome {
    pub restored_jsonl_files: usize,
    pub restored_state_rows: usize,
    pub skipped_reason: Option<String>,
}

fn official_history_unify_backup_parent() -> PathBuf {
    get_app_config_dir()
        .join("backups")
        .join(OFFICIAL_UNIFY_MIGRATION_NAME)
}

pub fn has_codex_official_history_unify_backup() -> bool {
    has_official_history_unify_backup_for_dir(
        &official_history_unify_backup_parent(),
        &canonical_dir_string(&get_codex_config_dir()),
    )
}

fn has_official_history_unify_backup_for_dir(ledger_parent: &Path, codex_dir_key: &str) -> bool {
    let Ok(entries) = fs::read_dir(ledger_parent) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let generation = entry.path();
        generation.is_dir() && backup_generation_matches_dir(&generation, codex_dir_key)
    })
}

/// 关闭统一会话开关时的可选还原：按迁移备份账本，把当时迁入共享 custom 桶的
/// 官方会话精确翻回 "openai" 桶。
pub fn restore_codex_official_history_from_backups(
) -> Result<CodexOfficialHistoryRestoreOutcome, AppError> {
    let _op_guard = lock_codex_official_history_op();
    if crate::settings::unify_codex_session_history() {
        return Ok(CodexOfficialHistoryRestoreOutcome {
            skipped_reason: Some("unify_toggle_on".to_string()),
            ..Default::default()
        });
    }
    let config_text = read_codex_config_text().unwrap_or_default();
    restore_codex_official_history_inner(
        &get_codex_config_dir(),
        &official_history_unify_backup_parent(),
        &migration_backup_root(OFFICIAL_UNIFY_RESTORE_BACKUP_NAME),
        &config_text,
    )
}

fn restore_codex_official_history_inner(
    codex_dir: &Path,
    ledger_parent: &Path,
    restore_backup_root: &Path,
    config_text: &str,
) -> Result<CodexOfficialHistoryRestoreOutcome, AppError> {
    let codex_dir_key = canonical_dir_string(codex_dir);
    let (official_session_ids, official_thread_ids) =
        collect_official_ledger(ledger_parent, &codex_dir_key)?;
    if official_session_ids.is_empty() && official_thread_ids.is_empty() {
        return Ok(CodexOfficialHistoryRestoreOutcome {
            skipped_reason: Some("no_backup_ledger".to_string()),
            ..Default::default()
        });
    }

    let mut files = Vec::new();
    collect_jsonl_files(&codex_dir.join("sessions"), &mut files, 0, 8);
    collect_jsonl_files(&codex_dir.join("archived_sessions"), &mut files, 0, 4);
    let mut restored_jsonl_files = 0;
    for file_path in files {
        if rewrite_codex_session_file_lines(&file_path, codex_dir, restore_backup_root, |line| {
            rewrite_codex_session_meta_line_for_restore(line, &official_session_ids)
        })? {
            restored_jsonl_files += 1;
        }
    }

    let mut restored_state_rows = 0;
    for db_path in codex_state_db_paths(codex_dir, config_text) {
        restored_state_rows += restore_codex_state_db_official_threads(
            &db_path,
            codex_dir,
            &official_thread_ids,
            restore_backup_root,
        )?;
    }

    if restored_jsonl_files == 0 && restored_state_rows == 0 {
        return Ok(CodexOfficialHistoryRestoreOutcome {
            skipped_reason: Some("nothing_to_restore".to_string()),
            ..Default::default()
        });
    }

    Ok(CodexOfficialHistoryRestoreOutcome {
        restored_jsonl_files,
        restored_state_rows,
        skipped_reason: None,
    })
}

fn collect_official_ledger(
    ledger_parent: &Path,
    codex_dir_key: &str,
) -> Result<(HashSet<String>, BTreeSet<String>), AppError> {
    let mut session_ids = HashSet::new();
    let mut thread_ids = BTreeSet::new();
    let entries = match fs::read_dir(ledger_parent) {
        Ok(entries) => entries,
        Err(_) => return Ok((session_ids, thread_ids)),
    };
    for entry in entries.flatten() {
        let generation = entry.path();
        if !generation.is_dir() {
            continue;
        }
        if !backup_generation_matches_dir(&generation, codex_dir_key) {
            continue;
        }
        let mut backup_files = Vec::new();
        collect_jsonl_files(&generation.join("jsonl"), &mut backup_files, 0, 10);
        for backup_file in backup_files {
            collect_official_session_ids_from_backup(&backup_file, &mut session_ids);
        }
        let mut backup_dbs = Vec::new();
        collect_files_with_extension(&generation.join("state"), "sqlite", &mut backup_dbs, 0, 4);
        for backup_db in backup_dbs {
            collect_official_thread_ids_from_backup(&backup_db, &mut thread_ids);
        }
    }
    Ok((session_ids, thread_ids))
}

fn backup_generation_matches_dir(generation: &Path, codex_dir_key: &str) -> bool {
    let Ok(text) = fs::read_to_string(generation.join("meta.json")) else {
        return true;
    };
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("codexConfigDir")
                .and_then(Value::as_str)
                .map(|dir| dir == codex_dir_key)
        })
        .unwrap_or(true)
}

fn collect_official_session_ids_from_backup(path: &Path, session_ids: &mut HashSet<String>) {
    let Ok(content) = fs::read_to_string(path) else {
        log::debug!("Failed to read unify backup file {}", path.display());
        return;
    };
    for line in content.lines() {
        if !line.contains("\"session_meta\"") || !line.contains("\"model_provider\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if payload.get("model_provider").and_then(Value::as_str)
            != Some(OFFICIAL_OPENAI_CODEX_MODEL_PROVIDER_ID)
        {
            continue;
        }
        if let Some(session_id) = payload.get("id").and_then(Value::as_str) {
            session_ids.insert(session_id.to_string());
        }
    }
}

fn collect_official_thread_ids_from_backup(db_path: &Path, thread_ids: &mut BTreeSet<String>) {
    let conn =
        match Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
            Ok(conn) => conn,
            Err(err) => {
                log::debug!(
                    "Failed to open unify backup state DB {}: {err}",
                    db_path.display()
                );
                return;
            }
        };
    let has_threads = Database::table_exists(&conn, "threads").unwrap_or(false)
        && Database::has_column(&conn, "threads", "model_provider").unwrap_or(false);
    if !has_threads {
        return;
    }
    let Ok(mut stmt) = conn.prepare("SELECT id FROM threads WHERE model_provider = ?1") else {
        return;
    };
    let Ok(rows) = stmt.query_map([OFFICIAL_OPENAI_CODEX_MODEL_PROVIDER_ID], |row| {
        row.get::<_, String>(0)
    }) else {
        return;
    };
    for thread_id in rows.flatten() {
        thread_ids.insert(thread_id);
    }
}

fn collect_files_with_extension(
    dir: &Path,
    extension: &str,
    files: &mut Vec<PathBuf>,
    depth: u8,
    max_depth: u8,
) {
    if depth > max_depth || !dir.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_with_extension(&path, extension, files, depth + 1, max_depth);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some(extension) {
            files.push(path);
        }
    }
}

fn rewrite_codex_session_meta_line_for_restore(
    line: &str,
    official_session_ids: &HashSet<String>,
) -> Option<String> {
    if !line.contains("\"session_meta\"") || !line.contains("\"model_provider\"") {
        return None;
    }
    let mut value: Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = value.get_mut("payload")?.as_object_mut()?;
    if payload.get("model_provider")?.as_str()? != CC_SWITCH_CODEX_MODEL_PROVIDER_ID {
        return None;
    }
    let session_id = payload.get("id")?.as_str()?;
    if !official_session_ids.contains(session_id) {
        return None;
    }
    payload.insert(
        "model_provider".to_string(),
        Value::String(OFFICIAL_OPENAI_CODEX_MODEL_PROVIDER_ID.to_string()),
    );
    serde_json::to_string(&value).ok()
}

fn restore_codex_state_db_official_threads(
    db_path: &Path,
    codex_dir: &Path,
    official_thread_ids: &BTreeSet<String>,
    backup_root: &Path,
) -> Result<usize, AppError> {
    if !db_path.exists() || official_thread_ids.is_empty() {
        return Ok(0);
    }

    let mut conn = Connection::open(db_path)
        .map_err(|e| AppError::Database(format!("打开 Codex state DB 失败: {e}")))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| AppError::Database(format!("设置 Codex state DB busy_timeout 失败: {e}")))?;

    if !Database::table_exists(&conn, "threads")?
        || !Database::has_column(&conn, "threads", "model_provider")?
    {
        return Ok(0);
    }

    let ids: Vec<&String> = official_thread_ids.iter().collect();
    let mut matching_rows: i64 = 0;
    for chunk in ids.chunks(STATE_DB_ID_CHUNK) {
        let placeholders = placeholders(chunk.len());
        let count_sql = format!(
            "SELECT COUNT(*) FROM threads WHERE model_provider = ? AND id IN ({placeholders})"
        );
        let mut values = Vec::with_capacity(chunk.len() + 1);
        values.push(CC_SWITCH_CODEX_MODEL_PROVIDER_ID.to_string());
        values.extend(chunk.iter().map(|id| (*id).clone()));
        let count: i64 = conn
            .query_row(&count_sql, params_from_iter(values.iter()), |row| {
                row.get(0)
            })
            .map_err(|e| AppError::Database(format!("统计 Codex state DB 待还原行失败: {e}")))?;
        matching_rows += count;
    }
    if matching_rows == 0 {
        return Ok(0);
    }

    backup_codex_state_db(db_path, codex_dir, backup_root, &conn)?;

    let tx = conn
        .transaction()
        .map_err(|e| AppError::Database(format!("开启 Codex state DB 还原事务失败: {e}")))?;
    let mut changed = 0;
    for chunk in ids.chunks(STATE_DB_ID_CHUNK) {
        let placeholders = placeholders(chunk.len());
        let update_sql = format!(
            "UPDATE threads SET model_provider = ? WHERE model_provider = ? AND id IN ({placeholders})"
        );
        let mut values = Vec::with_capacity(chunk.len() + 2);
        values.push(OFFICIAL_OPENAI_CODEX_MODEL_PROVIDER_ID.to_string());
        values.push(CC_SWITCH_CODEX_MODEL_PROVIDER_ID.to_string());
        values.extend(chunk.iter().map(|id| (*id).clone()));
        changed += tx
            .execute(&update_sql, params_from_iter(values.iter()))
            .map_err(|e| AppError::Database(format!("还原 Codex state DB provider 失败: {e}")))?;
    }
    tx.commit()
        .map_err(|e| AppError::Database(format!("提交 Codex state DB 还原事务失败: {e}")))?;
    Ok(changed)
}

fn migrate_codex_provider_templates_to_custom(
    db: &Database,
    backup_root: &Path,
) -> Result<CodexProviderTemplateBucketMigrationOutcome, AppError> {
    let providers = db.get_all_providers("codex")?;
    let common_config_snippet = db.get_config_snippet("codex")?;
    let mut migrated_provider_ids = Vec::new();

    for (_, provider) in providers {
        if provider.category.as_deref() == Some("official")
            || is_official_seed_id(&provider.id)
            || provider.is_codex_oauth()
        {
            continue;
        }

        let Some(config_text) = provider
            .settings_config
            .get("config")
            .and_then(|value| value.as_str())
        else {
            continue;
        };

        let source_provider_ids =
            provider_migration_source_ids(&provider, config_text, common_config_snippet.as_deref());
        if source_provider_ids.is_empty() {
            continue;
        }

        let Some(mut migrated_config_text) =
            migrate_provider_config_template_to_custom(config_text, &source_provider_ids)?
        else {
            continue;
        };
        if legacy_cli_deeplink_model_provider_id(&provider, config_text).is_some() {
            migrated_config_text =
                add_missing_custom_provider_name(&migrated_config_text, provider.name.as_str())?;
        }

        let mut settings = provider.settings_config.clone();
        let Some(obj) = settings.as_object_mut() else {
            log::warn!(
                "Skipping Codex provider template migration for {}: settings_config is not an object",
                provider.id
            );
            continue;
        };
        backup_provider_settings_config(&provider.id, &provider.settings_config, backup_root)?;
        obj.insert("config".to_string(), Value::String(migrated_config_text));
        db.update_provider_settings_config("codex", &provider.id, &settings)?;
        migrated_provider_ids.push(provider.id);
    }

    Ok(CodexProviderTemplateBucketMigrationOutcome {
        migrated_provider_ids,
        skipped_reason: None,
    })
}

fn collect_source_model_provider_ids(db: &Database) -> Result<BTreeSet<String>, AppError> {
    let providers = db.get_all_providers("codex")?;
    let common_config_snippet = db.get_config_snippet("codex")?;
    let mut ids = BTreeSet::new();

    for provider in providers.values() {
        if provider.category.as_deref() == Some("official")
            || is_official_seed_id(&provider.id)
            || provider.is_codex_oauth()
        {
            continue;
        }

        insert_known_cc_switch_legacy_source_id(&mut ids, &provider.id);

        let Some(raw_config_text) = provider
            .settings_config
            .get("config")
            .and_then(|value| value.as_str())
        else {
            continue;
        };
        ids.extend(provider_migration_source_ids(
            provider,
            raw_config_text,
            common_config_snippet.as_deref(),
        ));
        if let Some(provider_id) =
            legacy_codex_model_provider_id_from_normalized_config(raw_config_text)
        {
            insert_known_cc_switch_legacy_source_id(&mut ids, &provider_id);
        }
    }

    Ok(ids)
}

/// Upstream trusts its fixed preset ids. Older cc-switch-cli builds additionally
/// generated a provider id from the local provider key, so that one exact
/// generated shape is included as the only CLI-specific migration source.
fn provider_migration_source_ids(
    provider: &crate::provider::Provider,
    raw_config_text: &str,
    common_config_snippet: Option<&str>,
) -> BTreeSet<String> {
    let effective_config_text =
        crate::services::provider::ProviderService::build_effective_live_snapshot(
            &crate::app_config::AppType::Codex,
            provider,
            common_config_snippet,
            true,
        )
        .ok()
        .and_then(|effective| {
            effective
                .get("config")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| raw_config_text.to_string());

    let mut ids = trusted_legacy_codex_model_provider_ids_from_config(raw_config_text);
    ids.extend(trusted_legacy_codex_model_provider_ids_from_config(
        &effective_config_text,
    ));
    for config_text in [raw_config_text, effective_config_text.as_str()] {
        if let Some(provider_id) =
            cli_generated_quick_config_model_provider_id(provider, config_text)
        {
            ids.insert(provider_id);
        }
        if let Some(provider_id) = legacy_full_deeplink_model_provider_id(provider, config_text) {
            ids.insert(provider_id);
        }
    }
    if let Some(provider_id) = legacy_cli_deeplink_model_provider_id(provider, raw_config_text) {
        ids.insert(provider_id);
    }
    if let Some(provider_id) =
        legacy_flat_deeplink_conversion_model_provider_id(provider, raw_config_text)
    {
        ids.insert(provider_id);
    }
    ids
}

fn insert_known_cc_switch_legacy_source_id(ids: &mut BTreeSet<String>, provider_id: &str) {
    let trimmed = provider_id.trim();
    if is_known_cc_switch_legacy_codex_model_provider_id(trimmed) {
        ids.insert(trimmed.to_string());
    }
}

fn migration_backup_root(migration_name: &str) -> PathBuf {
    get_app_config_dir()
        .join("backups")
        .join(migration_name)
        .join(Local::now().format("%Y%m%d_%H%M%S").to_string())
}

fn is_known_cc_switch_legacy_codex_model_provider_id(provider_id: &str) -> bool {
    CC_SWITCH_LEGACY_CODEX_MODEL_PROVIDER_IDS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(provider_id))
}

fn legacy_codex_model_provider_id_from_normalized_config(config_text: &str) -> Option<String> {
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let provider_id = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)?;
    if provider_id != CC_SWITCH_CODEX_MODEL_PROVIDER_ID
        && provider_id != LEGACY_CC_SWITCH_CODEX_MODEL_PROVIDER_ID
    {
        return None;
    }

    let name = doc
        .get("model_providers")
        .and_then(|item| item.as_table())
        .and_then(|table| table.get(provider_id))
        .and_then(|item| item.as_table())
        .and_then(|table| table.get("name"))
        .and_then(|item| item.as_str())?
        .trim();

    normalized_legacy_codex_provider_name(name).map(str::to_string)
}

fn normalized_legacy_codex_provider_name(name: &str) -> Option<&'static str> {
    if is_known_cc_switch_legacy_codex_model_provider_id(name) {
        return CC_SWITCH_LEGACY_CODEX_MODEL_PROVIDER_IDS
            .iter()
            .copied()
            .find(|known| known.eq_ignore_ascii_case(name));
    }

    match name {
        "E-FlowCode" => Some("eflowcode"),
        "PIPELLM" => Some("pipellm"),
        _ => None,
    }
}

fn trusted_legacy_codex_model_provider_ids_from_config(config_text: &str) -> BTreeSet<String> {
    let Ok(doc) = config_text.parse::<DocumentMut>() else {
        return BTreeSet::new();
    };

    trusted_legacy_codex_model_provider_ids_from_doc(&doc)
}

fn trusted_legacy_codex_model_provider_ids_from_doc(doc: &DocumentMut) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    insert_trusted_legacy_config_model_provider_id(&mut ids, doc, doc.get("model_provider"));

    if let Some(profiles) = doc.get("profiles").and_then(|item| item.as_table_like()) {
        for (_, profile_item) in profiles.iter() {
            if let Some(profile_table) = profile_item.as_table_like() {
                insert_trusted_legacy_config_model_provider_id(
                    &mut ids,
                    doc,
                    profile_table.get("model_provider"),
                );
            }
        }
    }

    ids
}

fn insert_trusted_legacy_config_model_provider_id(
    ids: &mut BTreeSet<String>,
    doc: &DocumentMut,
    item: Option<&toml_edit::Item>,
) {
    let Some(provider_id) = item.and_then(|item| item.as_str()).map(str::trim) else {
        return;
    };
    if provider_id.is_empty()
        || !is_known_cc_switch_legacy_codex_model_provider_id(provider_id)
        || !config_defines_model_provider(doc, provider_id)
    {
        return;
    }
    ids.insert(provider_id.to_string());
}

fn config_defines_model_provider(doc: &DocumentMut, provider_id: &str) -> bool {
    doc.get("model_providers")
        .and_then(|item| item.as_table())
        .and_then(|table| table.get(provider_id))
        .and_then(|item| item.as_table())
        .is_some()
}

/// Detect the dynamic provider ids emitted by older cc-switch-cli generators.
///
/// Upstream only needs its fixed preset ids. Older CLI builds also derived this
/// id from the local provider key, so accept that one generated shape here.
fn cli_generated_quick_config_model_provider_id(
    provider: &crate::provider::Provider,
    config_text: &str,
) -> Option<String> {
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let active_provider_id = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)?;
    let stored_provider_id = provider.id.trim();
    if stored_provider_id.is_empty() || stored_provider_id == "default" {
        return None;
    }
    let expected_provider_id = crate::codex_config::clean_codex_provider_key(stored_provider_id);
    if active_provider_id != expected_provider_id {
        return None;
    }
    if expected_provider_id == CC_SWITCH_CODEX_MODEL_PROVIDER_ID
        || !crate::codex_config::is_custom_codex_model_provider_id(&expected_provider_id)
    {
        return None;
    }

    let provider_table = doc
        .get("model_providers")
        .and_then(|item| item.as_table())
        .and_then(|table| table.get(&expected_provider_id))
        .and_then(|item| item.as_table())?;
    let table_name = provider_table
        .get("name")
        .and_then(|item| item.as_str())
        .map(str::trim)?;
    if table_name != expected_provider_id && table_name != "OpenAI" {
        return None;
    }

    let has_generated_shape = doc
        .get("model")
        .and_then(|item| item.as_str())
        .is_some_and(|model| !model.trim().is_empty())
        && doc
            .get("model_reasoning_effort")
            .and_then(|item| item.as_str())
            == Some("high")
        && doc
            .get("disable_response_storage")
            .and_then(|item| item.as_bool())
            == Some(true)
        && provider_table
            .get("base_url")
            .and_then(|item| item.as_str())
            .is_some_and(|base_url| !base_url.trim().is_empty())
        && provider_table
            .get("wire_api")
            .and_then(|item| item.as_str())
            .is_some_and(|wire_api| matches!(wire_api, "responses" | "chat"))
        && provider_table
            .get("requires_openai_auth")
            .and_then(|item| item.as_bool())
            .is_some();

    has_generated_shape.then_some(expected_provider_id)
}

/// Detect Codex configs emitted by cc-switch-cli's historical deep-link path.
///
/// That implementation used `<sanitized-name>-<timestamp_ms>` as the database
/// id, but a name-derived Codex routing id. The timestamp relation and exact
/// provider-table shape keep this narrower than accepting arbitrary
/// user-authored provider ids. The current display name is deliberately not
/// part of the proof because provider editing can change it without changing
/// the stored id or config.
fn legacy_cli_deeplink_model_provider_id(
    provider: &crate::provider::Provider,
    config_text: &str,
) -> Option<String> {
    let id_name_prefix = legacy_cli_deeplink_id_prefix(provider)?;
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let active_provider_id = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)?;
    if active_provider_id == CC_SWITCH_CODEX_MODEL_PROVIDER_ID
        || !crate::codex_config::is_custom_codex_model_provider_id(active_provider_id)
    {
        return None;
    }
    let active_name_fingerprint = active_provider_id
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let id_name_fingerprint = id_name_prefix
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if active_name_fingerprint.is_empty() || active_name_fingerprint != id_name_fingerprint {
        return None;
    }

    if doc
        .get("model")
        .and_then(|item| item.as_str())
        .is_none_or(|model| model.trim().is_empty())
        || doc.get("model_reasoning_effort").is_some()
        || doc.get("disable_response_storage").is_some()
    {
        return None;
    }

    let provider_table = doc
        .get("model_providers")
        .and_then(|item| item.as_table())
        .and_then(|table| table.get(active_provider_id))
        .and_then(|item| item.as_table())?;
    let has_legacy_deeplink_shape = provider_table.get("name").is_none()
        && provider_table
            .get("base_url")
            .and_then(|item| item.as_str())
            .is_some_and(|base_url| !base_url.trim().is_empty())
        && provider_table
            .get("wire_api")
            .and_then(|item| item.as_str())
            == Some("responses")
        && provider_table
            .get("requires_openai_auth")
            .and_then(|item| item.as_bool())
            == Some(false)
        && provider_table.get("env_key").and_then(|item| item.as_str()) == Some("OPENAI_API_KEY")
        && provider
            .settings_config
            .get("auth")
            .and_then(Value::as_object)
            .is_some_and(|auth| auth.contains_key("OPENAI_API_KEY"));

    has_legacy_deeplink_shape.then(|| active_provider_id.to_string())
}

/// Detect the first full Codex config emitted by cc-switch-cli's deep-link
/// importer. It used the same timestamped database id as the later importer,
/// but retained the earlier generated provider-table defaults.
fn legacy_full_deeplink_model_provider_id(
    provider: &crate::provider::Provider,
    config_text: &str,
) -> Option<String> {
    let id_name_prefix = legacy_cli_deeplink_id_prefix(provider)?;
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let active_provider_id = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)?;
    if active_provider_id == CC_SWITCH_CODEX_MODEL_PROVIDER_ID
        || !crate::codex_config::is_custom_codex_model_provider_id(active_provider_id)
    {
        return None;
    }
    let active_name_fingerprint = active_provider_id
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let id_name_fingerprint = id_name_prefix
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if active_name_fingerprint.is_empty() || active_name_fingerprint != id_name_fingerprint {
        return None;
    }

    if doc
        .get("model")
        .and_then(|item| item.as_str())
        .is_none_or(|model| model.trim().is_empty())
        || doc
            .get("model_reasoning_effort")
            .and_then(|item| item.as_str())
            != Some("high")
        || doc
            .get("disable_response_storage")
            .and_then(|item| item.as_bool())
            != Some(true)
    {
        return None;
    }

    let provider_table = doc
        .get("model_providers")
        .and_then(|item| item.as_table())
        .and_then(|table| table.get(active_provider_id))
        .and_then(|item| item.as_table())?;
    let has_legacy_deeplink_shape = provider_table
        .get("name")
        .and_then(|item| item.as_str())
        .map(str::trim)
        == Some(active_provider_id)
        && provider_table
            .get("base_url")
            .and_then(|item| item.as_str())
            .is_some_and(|base_url| !base_url.trim().is_empty())
        && provider_table
            .get("wire_api")
            .and_then(|item| item.as_str())
            == Some("responses")
        && provider_table
            .get("requires_openai_auth")
            .and_then(|item| item.as_bool())
            == Some(true)
        && provider_table.get("env_key").is_none()
        && provider
            .settings_config
            .get("auth")
            .and_then(Value::as_object)
            .is_some_and(|auth| auth.contains_key("OPENAI_API_KEY"));

    has_legacy_deeplink_shape.then(|| active_provider_id.to_string())
}

/// Detect the older deep-link cohort that first stored a flat config and was
/// later rewritten by the v4.7.4 startup migration. That converter derived the
/// routing id from the immutable timestamped database id.
fn legacy_flat_deeplink_conversion_model_provider_id(
    provider: &crate::provider::Provider,
    config_text: &str,
) -> Option<String> {
    legacy_cli_deeplink_id_prefix(provider)?;
    let expected_provider_id = crate::codex_config::clean_codex_provider_key(&provider.id);
    if expected_provider_id == CC_SWITCH_CODEX_MODEL_PROVIDER_ID
        || !crate::codex_config::is_custom_codex_model_provider_id(&expected_provider_id)
    {
        return None;
    }

    let doc = config_text.parse::<DocumentMut>().ok()?;
    if doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)
        != Some(expected_provider_id.as_str())
        || doc
            .get("model")
            .and_then(|item| item.as_str())
            .is_none_or(|model| model.trim().is_empty())
        || doc.get("model_reasoning_effort").is_some()
        || doc.get("disable_response_storage").is_some()
    {
        return None;
    }

    let provider_table = doc
        .get("model_providers")
        .and_then(|item| item.as_table())
        .and_then(|table| table.get(&expected_provider_id))
        .and_then(|item| item.as_table())?;
    let has_legacy_conversion_shape = provider_table
        .get("name")
        .and_then(|item| item.as_str())
        .map(str::trim)
        == Some(expected_provider_id.as_str())
        && provider_table
            .get("base_url")
            .and_then(|item| item.as_str())
            .is_some_and(|base_url| !base_url.trim().is_empty())
        && provider_table
            .get("wire_api")
            .and_then(|item| item.as_str())
            == Some("responses")
        && provider_table
            .get("requires_openai_auth")
            .and_then(|item| item.as_bool())
            == Some(true)
        && provider_table.get("env_key").is_none()
        && provider
            .settings_config
            .get("auth")
            .and_then(Value::as_object)
            .is_some_and(|auth| auth.contains_key("OPENAI_API_KEY"));

    has_legacy_conversion_shape.then_some(expected_provider_id)
}

fn legacy_cli_deeplink_id_prefix(provider: &crate::provider::Provider) -> Option<&str> {
    let (name_prefix, timestamp_text) = provider.id.rsplit_once('-')?;
    if name_prefix.is_empty()
        || timestamp_text.len() != 13
        || !timestamp_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let timestamp = timestamp_text.parse::<i64>().ok()?;
    // Released deep-link imports stored created_at as NULL. Newer records may
    // carry it, in which case it remains an additional timestamp cross-check.
    if provider
        .created_at
        .is_some_and(|created_at| timestamp.abs_diff(created_at) > 60_000)
    {
        return None;
    }
    Some(name_prefix)
}

fn add_missing_custom_provider_name(
    config_text: &str,
    provider_name: &str,
) -> Result<String, AppError> {
    let mut doc = config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?;
    let Some(custom_table) = doc
        .get_mut("model_providers")
        .and_then(|item| item.as_table_mut())
        .and_then(|table| table.get_mut(CC_SWITCH_CODEX_MODEL_PROVIDER_ID))
        .and_then(|item| item.as_table_mut())
    else {
        return Ok(config_text.to_string());
    };
    if custom_table
        .get("name")
        .and_then(|item| item.as_str())
        .is_none_or(|name| name.trim().is_empty())
    {
        let display_name = if provider_name.trim().is_empty() {
            CC_SWITCH_CODEX_MODEL_PROVIDER_ID
        } else {
            provider_name.trim()
        };
        custom_table["name"] = toml_edit::value(display_name);
    }
    Ok(doc.to_string())
}

fn migrate_provider_config_template_to_custom(
    config_text: &str,
    source_provider_ids: &BTreeSet<String>,
) -> Result<Option<String>, AppError> {
    if config_text.trim().is_empty() {
        return Ok(None);
    }

    let mut doc = config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?;

    if source_provider_ids.is_empty() {
        return Ok(None);
    }

    let active_provider_id = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|provider_id| !provider_id.is_empty())
        .map(str::to_string);

    let custom_table_exists =
        config_defines_model_provider(&doc, CC_SWITCH_CODEX_MODEL_PROVIDER_ID);
    let source_provider_id_to_move = active_provider_id
        .as_deref()
        .filter(|provider_id| source_provider_ids.contains(*provider_id))
        .map(str::to_string)
        .or_else(|| {
            if custom_table_exists {
                None
            } else {
                source_provider_ids.iter().next().cloned()
            }
        });

    let mut changed = false;

    if let Some(source_provider_id) = source_provider_id_to_move {
        let Some(model_providers) = doc
            .get_mut("model_providers")
            .and_then(|item| item.as_table_mut())
        else {
            return Ok(None);
        };

        let Some(provider_table) = model_providers.remove(source_provider_id.as_str()) else {
            return Ok(None);
        };
        model_providers[CC_SWITCH_CODEX_MODEL_PROVIDER_ID] = provider_table;
        changed = true;
    }

    if active_provider_id
        .as_deref()
        .is_some_and(|provider_id| source_provider_ids.contains(provider_id))
    {
        doc["model_provider"] = toml_edit::value(CC_SWITCH_CODEX_MODEL_PROVIDER_ID);
        changed = true;
    }

    for source_provider_id in source_provider_ids {
        if rewrite_legacy_provider_profile_refs(&mut doc, source_provider_id.as_str()) {
            changed = true;
        }
    }

    if changed {
        Ok(Some(doc.to_string()))
    } else {
        Ok(None)
    }
}

fn rewrite_legacy_provider_profile_refs(doc: &mut DocumentMut, source_provider_id: &str) -> bool {
    let Some(profiles) = doc
        .get_mut("profiles")
        .and_then(|item| item.as_table_like_mut())
    else {
        return false;
    };

    let mut changed = false;
    let profile_keys: Vec<String> = profiles.iter().map(|(key, _)| key.to_string()).collect();
    for profile_key in profile_keys {
        let Some(profile_table) = profiles
            .get_mut(&profile_key)
            .and_then(|item| item.as_table_like_mut())
        else {
            continue;
        };

        let references_legacy = profile_table
            .get("model_provider")
            .and_then(|item| item.as_str())
            == Some(source_provider_id);
        if references_legacy {
            profile_table.insert(
                "model_provider",
                toml_edit::value(CC_SWITCH_CODEX_MODEL_PROVIDER_ID),
            );
            changed = true;
        }
    }
    changed
}

fn migrate_codex_jsonl_files(
    codex_dir: &Path,
    source_provider_ids: &BTreeSet<String>,
    backup_root: &Path,
) -> Result<usize, AppError> {
    let mut files = Vec::new();
    collect_jsonl_files(&codex_dir.join("sessions"), &mut files, 0, 8);
    collect_jsonl_files(&codex_dir.join("archived_sessions"), &mut files, 0, 4);

    let source_provider_ids: HashSet<String> = source_provider_ids.iter().cloned().collect();
    let mut migrated = 0;
    for file_path in files {
        if rewrite_codex_session_file_for_provider_bucket(
            &file_path,
            codex_dir,
            &source_provider_ids,
            backup_root,
        )? {
            migrated += 1;
        }
    }
    Ok(migrated)
}

fn collect_jsonl_files(dir: &Path, files: &mut Vec<PathBuf>, depth: u8, max_depth: u8) {
    if depth > max_depth || !dir.is_dir() {
        return;
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            log::debug!(
                "Failed to read Codex session directory {}: {err}",
                dir.display()
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, files, depth + 1, max_depth);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
}

fn rewrite_codex_session_file_for_provider_bucket(
    path: &Path,
    codex_dir: &Path,
    source_provider_ids: &HashSet<String>,
    backup_root: &Path,
) -> Result<bool, AppError> {
    rewrite_codex_session_file_lines(path, codex_dir, backup_root, |line| {
        rewrite_codex_session_meta_line(line, source_provider_ids)
    })
}

fn rewrite_codex_session_file_lines(
    path: &Path,
    codex_dir: &Path,
    backup_root: &Path,
    rewrite_line: impl Fn(&str) -> Option<String>,
) -> Result<bool, AppError> {
    let metadata_before = fs::metadata(path).map_err(|e| AppError::io(path, e))?;
    let modified_before = metadata_before.modified().ok();
    let len_before = metadata_before.len();
    let content = fs::read_to_string(path).map_err(|e| AppError::io(path, e))?;

    let mut rewritten = String::with_capacity(content.len());
    let mut changed = false;
    for segment in content.split_inclusive('\n') {
        let (line, newline) = segment
            .strip_suffix('\n')
            .map(|line| (line, "\n"))
            .unwrap_or((segment, ""));
        if let Some(next_line) = rewrite_line(line) {
            rewritten.push_str(&next_line);
            changed = true;
        } else {
            rewritten.push_str(line);
        }
        rewritten.push_str(newline);
    }

    if !changed {
        return Ok(false);
    }

    ensure_codex_session_file_unchanged(path, modified_before, len_before)?;
    backup_codex_jsonl_file(path, codex_dir, backup_root)?;
    ensure_codex_session_file_unchanged(path, modified_before, len_before)?;
    atomic_write(path, rewritten.as_bytes())?;
    Ok(true)
}

fn ensure_codex_session_file_unchanged(
    path: &Path,
    modified_before: Option<SystemTime>,
    len_before: u64,
) -> Result<(), AppError> {
    let metadata_after = fs::metadata(path).map_err(|e| AppError::io(path, e))?;
    if metadata_after.modified().ok() != modified_before || metadata_after.len() != len_before {
        return Err(AppError::Message(format!(
            "Codex session file changed during migration: {}",
            path.display()
        )));
    }
    Ok(())
}

fn rewrite_codex_session_meta_line(
    line: &str,
    source_provider_ids: &HashSet<String>,
) -> Option<String> {
    if !line.contains("\"session_meta\"") || !line.contains("\"model_provider\"") {
        return None;
    }

    let mut value: Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }

    let payload = value.get_mut("payload")?.as_object_mut()?;
    let current_provider = payload.get("model_provider")?.as_str()?;
    if !source_provider_ids.contains(current_provider) {
        return None;
    }

    payload.insert(
        "model_provider".to_string(),
        Value::String(CC_SWITCH_CODEX_MODEL_PROVIDER_ID.to_string()),
    );
    serde_json::to_string(&value).ok()
}

fn migrate_codex_state_dbs(
    codex_dir: &Path,
    source_provider_ids: &BTreeSet<String>,
    backup_root: &Path,
) -> Result<usize, AppError> {
    let config_text = read_codex_config_text().unwrap_or_default();
    let mut migrated = 0;
    for db_path in codex_state_db_paths(codex_dir, &config_text) {
        migrated += migrate_codex_state_db_provider_bucket(
            &db_path,
            codex_dir,
            source_provider_ids,
            backup_root,
        )?;
    }
    Ok(migrated)
}

fn migrate_codex_state_db_provider_bucket(
    db_path: &Path,
    codex_dir: &Path,
    source_provider_ids: &BTreeSet<String>,
    backup_root: &Path,
) -> Result<usize, AppError> {
    if !db_path.exists() || source_provider_ids.is_empty() {
        return Ok(0);
    }

    let mut conn = Connection::open(db_path)
        .map_err(|e| AppError::Database(format!("打开 Codex state DB 失败: {e}")))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| AppError::Database(format!("设置 Codex state DB busy_timeout 失败: {e}")))?;

    if !Database::table_exists(&conn, "threads")?
        || !Database::has_column(&conn, "threads", "model_provider")?
    {
        return Ok(0);
    }

    let placeholders = placeholders(source_provider_ids.len());
    let count_sql =
        format!("SELECT COUNT(*) FROM threads WHERE model_provider IN ({placeholders})");
    let matching_rows: i64 = conn
        .query_row(
            &count_sql,
            params_from_iter(source_provider_ids.iter()),
            |row| row.get(0),
        )
        .map_err(|e| AppError::Database(format!("统计 Codex state DB 待迁移行失败: {e}")))?;
    if matching_rows == 0 {
        return Ok(0);
    }

    backup_codex_state_db(db_path, codex_dir, backup_root, &conn)?;

    let update_sql =
        format!("UPDATE threads SET model_provider = ? WHERE model_provider IN ({placeholders})");
    let mut values = Vec::with_capacity(source_provider_ids.len() + 1);
    values.push(CC_SWITCH_CODEX_MODEL_PROVIDER_ID.to_string());
    values.extend(source_provider_ids.iter().cloned());
    let tx = conn
        .transaction()
        .map_err(|e| AppError::Database(format!("开启 Codex state DB 迁移事务失败: {e}")))?;
    let changed = tx
        .execute(&update_sql, params_from_iter(values.iter()))
        .map_err(|e| AppError::Database(format!("迁移 Codex state DB provider 失败: {e}")))?;
    tx.commit()
        .map_err(|e| AppError::Database(format!("提交 Codex state DB 迁移事务失败: {e}")))?;
    Ok(changed)
}

fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

fn backup_codex_jsonl_file(
    path: &Path,
    codex_dir: &Path,
    backup_root: &Path,
) -> Result<(), AppError> {
    let backup_path = backup_root
        .join("jsonl")
        .join(relative_backup_path(path, codex_dir));
    copy_existing_file(path, &backup_path)
}

fn backup_codex_state_db(
    db_path: &Path,
    codex_dir: &Path,
    backup_root: &Path,
    source_conn: &Connection,
) -> Result<(), AppError> {
    let backup_path = backup_root
        .join("state")
        .join(relative_backup_path(db_path, codex_dir));
    create_managed_config_parent_dirs(&backup_path)?;
    // Reserve the destination ourselves on every platform. This guarantees that
    // SQLite never opens an existing backup and that failure cleanup only removes
    // a file created by this invocation.
    create_codex_state_backup_file(&backup_path)?;

    let mut backup_conn = match open_codex_state_backup_connection(&backup_path) {
        Ok(conn) => conn,
        Err(err) => {
            remove_codex_state_backup_artifacts(&backup_path);
            return Err(err);
        }
    };
    let backup_result = match Backup::new(source_conn, &mut backup_conn) {
        Ok(backup) => run_sqlite_backup_to_completion(&backup),
        Err(err) => Err(AppError::Database(format!(
            "初始化 Codex state DB 备份失败: {err}"
        ))),
    };
    drop(backup_conn);

    if let Err(err) = backup_result {
        remove_codex_state_backup_artifacts(&backup_path);
        return Err(AppError::Database(format!(
            "写入 Codex state DB 备份失败: {err}"
        )));
    }
    Ok(())
}

fn create_codex_state_backup_file(backup_path: &Path) -> Result<(), AppError> {
    match fs::symlink_metadata(backup_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(AppError::InvalidInput(format!(
                "Codex state DB 备份文件不能是符号链接: {}",
                backup_path.display()
            )));
        }
        Ok(meta) if meta.is_file() => {
            return Err(AppError::InvalidInput(format!(
                "Codex state DB 备份文件已存在: {}",
                backup_path.display()
            )));
        }
        Ok(_) => {
            return Err(AppError::InvalidInput(format!(
                "Codex state DB 备份路径不是普通文件: {}",
                backup_path.display()
            )));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(AppError::io(backup_path, err)),
    }

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    match options.open(backup_path) {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Err(AppError::InvalidInput(
            format!("Codex state DB 备份文件已存在: {}", backup_path.display()),
        )),
        Err(err) => Err(AppError::io(backup_path, err)),
    }
}

fn remove_codex_state_backup_artifacts(backup_path: &Path) {
    let mut artifacts = vec![backup_path.to_path_buf()];
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut artifact = backup_path.as_os_str().to_os_string();
        artifact.push(suffix);
        artifacts.push(PathBuf::from(artifact));
    }

    for artifact in artifacts {
        match fs::remove_file(&artifact) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => log::warn!(
                "Failed to remove incomplete Codex state backup {}: {err}",
                artifact.display()
            ),
        }
    }
}

fn open_codex_state_backup_connection(backup_path: &Path) -> Result<Connection, AppError> {
    let open_path = canonicalize_existing_parent(backup_path)?;
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;

    Connection::open_with_flags(&open_path, flags)
        .map_err(|e| AppError::Database(format!("创建 Codex state DB 备份失败: {e}")))
}

fn canonicalize_existing_parent(path: &Path) -> Result<PathBuf, AppError> {
    let Some(file_name) = path.file_name() else {
        return Err(AppError::InvalidInput(format!(
            "Codex state DB 备份路径缺少文件名: {}",
            path.display()
        )));
    };
    let parent = path
        .parent()
        .ok_or_else(|| AppError::InvalidInput(format!("无效路径: {}", path.display())))?;
    let parent = parent.canonicalize().map_err(|e| AppError::io(parent, e))?;
    Ok(parent.join(file_name))
}

fn backup_provider_settings_config(
    provider_id: &str,
    settings_config: &Value,
    backup_root: &Path,
) -> Result<(), AppError> {
    let backup_path = backup_root
        .join("providers")
        .join(provider_settings_backup_filename(provider_id));
    create_managed_config_parent_dirs(&backup_path)?;

    let payload = serde_json::json!({
        "providerId": provider_id,
        "settingsConfig": settings_config,
    });
    let bytes =
        serde_json::to_vec_pretty(&payload).map_err(|e| AppError::JsonSerialize { source: e })?;
    atomic_write(&backup_path, &bytes)
}

fn provider_settings_backup_filename(provider_id: &str) -> String {
    let safe_id: String = provider_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let safe_id = if safe_id.is_empty() {
        "provider".to_string()
    } else {
        safe_id
    };
    // Keep the hash stable across processes while avoiding collisions after sanitization.
    let digest = Sha256::digest(provider_id.as_bytes());
    let hash = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{hash}-{safe_id}.settings_config.json")
}

fn copy_existing_file(source: &Path, target: &Path) -> Result<(), AppError> {
    create_managed_config_parent_dirs(target)?;
    copy_file(source, target)
}

fn relative_backup_path(path: &Path, root: &Path) -> PathBuf {
    if let Ok(relative) = path.strip_prefix(root) {
        return relative.to_path_buf();
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    let hash = hasher.finish();
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    PathBuf::from("external").join(format!("{hash:016x}-{file_name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;
    use tempfile::tempdir;

    fn source_ids(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn migrate_provider_templates_for_test(
        db: &Database,
    ) -> (
        CodexProviderTemplateBucketMigrationOutcome,
        tempfile::TempDir,
    ) {
        let backup_dir = tempdir().expect("backup dir");
        let outcome = migrate_codex_provider_templates_to_custom(db, backup_dir.path())
            .expect("migrate template");
        (outcome, backup_dir)
    }

    #[test]
    fn simulates_local_codex_provider_bucket_migration_end_to_end() {
        let dir = tempdir().expect("tempdir");
        let codex_dir = dir.path().join(".codex");
        let backup_root = dir.path().join("backup");
        fs::create_dir_all(&codex_dir).expect("create codex dir");

        let db = Database::memory().expect("memory db");
        let providers = [
            Provider::with_id(
                "rightcode".to_string(),
                "RightCode".to_string(),
                serde_json::json!({
                    "auth": {},
                    "config": r#"model_provider = "aihubmix"

[model_providers.aihubmix]
name = "AIHubMix"
base_url = "https://aihubmix.example/v1"
"#
                }),
                None,
            ),
            Provider::with_id(
                "legacy-ccswitch".to_string(),
                "Legacy CC Switch".to_string(),
                serde_json::json!({
                    "auth": {},
                    "config": r#"model_provider = "ccswitch"

[model_providers.ccswitch]
name = "AIHubMix"
base_url = "https://aihubmix.example/v1"
"#
                }),
                None,
            ),
            Provider::with_id(
                "normalized-aihubmix".to_string(),
                "Already Normalized".to_string(),
                serde_json::json!({
                    "auth": {},
                    "config": r#"model_provider = "custom"

[model_providers.custom]
name = "AIHubMix"
base_url = "https://aihubmix.example/v1"
"#
                }),
                None,
            ),
            Provider::with_id(
                "manual-relay".to_string(),
                "Manual Relay".to_string(),
                serde_json::json!({
                    "auth": {},
                    "config": r#"model_provider = "my-private-relay"

[model_providers.my-private-relay]
name = "Manual Relay"
base_url = "http://localhost:8080/v1"
"#
                }),
                None,
            ),
            Provider::with_id(
                "custom-openai".to_string(),
                "Custom OpenAI".to_string(),
                serde_json::json!({
                    "auth": {},
                    "config": r#"model_provider = "openai"

[model_providers.openai]
name = "Custom OpenAI"
base_url = "https://proxy.example/v1"
"#
                }),
                None,
            ),
        ];
        for provider in providers {
            db.save_provider("codex", &provider).expect("save provider");
        }

        let mut official = Provider::with_id(
            "codex-official".to_string(),
            "OpenAI Official".to_string(),
            serde_json::json!({"auth": {}, "config": "model_provider = \"openai\""}),
            None,
        );
        official.category = Some("official".to_string());
        db.save_provider("codex", &official).expect("save official");

        let source_provider_ids = collect_source_model_provider_ids(&db).expect("collect ids");
        assert_eq!(
            source_provider_ids,
            source_ids(&["aihubmix", "ccswitch", "rightcode"])
        );

        let session_dir = codex_dir.join("sessions/2026/05/28");
        fs::create_dir_all(&session_dir).expect("create session dir");
        let session_path = session_dir.join("local-sim.jsonl");
        fs::write(
            &session_path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s1\",\"model_provider\":\"rightcode\"}}\n",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s2\",\"model_provider\":\"aihubmix\"}}\n",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s3\",\"model_provider\":\"ccswitch\"}}\n",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s4\",\"model_provider\":\"my-private-relay\"}}\n",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s5\",\"model_provider\":\"openai\"}}\n",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s6\",\"model_provider\":\"custom\"}}\n",
            ),
        )
        .expect("write session");

        let migrated_jsonl =
            migrate_codex_jsonl_files(&codex_dir, &source_provider_ids, &backup_root)
                .expect("migrate jsonl");
        assert_eq!(migrated_jsonl, 1);
        let session_text = fs::read_to_string(&session_path).expect("read session");
        assert_eq!(
            session_text
                .matches("\"model_provider\":\"custom\"")
                .count(),
            4
        );
        assert!(session_text.contains("\"model_provider\":\"my-private-relay\""));
        assert!(session_text.contains("\"model_provider\":\"openai\""));
        assert!(backup_root
            .join("jsonl/sessions/2026/05/28/local-sim.jsonl")
            .exists());

        let state_db_path = codex_dir.join(CODEX_STATE_DB_FILENAME);
        let conn = Connection::open(&state_db_path).expect("open state db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                model_provider TEXT NOT NULL
            );
            INSERT INTO threads (id, model_provider) VALUES
                ('rightcode-thread', 'rightcode'),
                ('aihubmix-thread', 'aihubmix'),
                ('ccswitch-thread', 'ccswitch'),
                ('manual-thread', 'my-private-relay'),
                ('openai-thread', 'openai'),
                ('custom-thread', 'custom');",
        )
        .expect("seed state db");
        drop(conn);

        let migrated_state_rows = migrate_codex_state_db_provider_bucket(
            &state_db_path,
            &codex_dir,
            &source_provider_ids,
            &backup_root,
        )
        .expect("migrate state db");
        assert_eq!(migrated_state_rows, 3);

        let conn = Connection::open(&state_db_path).expect("reopen state db");
        let count_provider = |provider_id: &str| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM threads WHERE model_provider = ?1",
                [provider_id],
                |row| row.get(0),
            )
            .expect("count provider")
        };
        assert_eq!(count_provider("custom"), 4);
        assert_eq!(count_provider("my-private-relay"), 1);
        assert_eq!(count_provider("openai"), 1);
        assert!(backup_root
            .join("state")
            .join(CODEX_STATE_DB_FILENAME)
            .exists());
        drop(conn);

        let template_outcome = migrate_codex_provider_templates_to_custom(&db, &backup_root)
            .expect("migrate provider templates");
        assert!(!template_outcome
            .migrated_provider_ids
            .iter()
            .any(|id| id == "normalized-aihubmix"));
        assert_eq!(
            source_ids(
                &template_outcome
                    .migrated_provider_ids
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            ),
            source_ids(&["legacy-ccswitch", "rightcode"])
        );

        let config_provider_id = |provider_id: &str| -> String {
            db.get_provider_by_id(provider_id, "codex")
                .expect("get provider")
                .expect("provider exists")
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .expect("config text")
                .to_string()
        };

        let rightcode_config: toml::Value =
            toml::from_str(&config_provider_id("rightcode")).expect("parse rightcode config");
        assert_eq!(
            rightcode_config
                .get("model_provider")
                .and_then(|value| value.as_str()),
            Some("custom")
        );
        assert!(rightcode_config
            .get("model_providers")
            .and_then(|value| value.get("aihubmix"))
            .is_none());

        let ccswitch_config: toml::Value =
            toml::from_str(&config_provider_id("legacy-ccswitch")).expect("parse ccswitch config");
        assert_eq!(
            ccswitch_config
                .get("model_provider")
                .and_then(|value| value.as_str()),
            Some("custom")
        );
        assert!(ccswitch_config
            .get("model_providers")
            .and_then(|value| value.get("ccswitch"))
            .is_none());

        let manual_config: toml::Value =
            toml::from_str(&config_provider_id("manual-relay")).expect("parse manual config");
        assert_eq!(
            manual_config
                .get("model_provider")
                .and_then(|value| value.as_str()),
            Some("my-private-relay")
        );

        let openai_config: toml::Value =
            toml::from_str(&config_provider_id("custom-openai")).expect("parse openai config");
        assert_eq!(
            openai_config
                .get("model_provider")
                .and_then(|value| value.as_str()),
            Some("openai")
        );

        let normalized_config: toml::Value =
            toml::from_str(&config_provider_id("normalized-aihubmix"))
                .expect("parse normalized config");
        assert_eq!(
            normalized_config
                .get("model_provider")
                .and_then(|value| value.as_str()),
            Some("custom")
        );
    }

    #[test]
    fn simulates_official_history_unify_migration_end_to_end() {
        let dir = tempdir().expect("tempdir");
        let codex_dir = dir.path().join(".codex");
        let backup_root = dir.path().join("backup");
        fs::create_dir_all(&codex_dir).expect("create codex dir");

        let source_provider_ids = source_ids(&[OFFICIAL_OPENAI_CODEX_MODEL_PROVIDER_ID]);

        let session_dir = codex_dir.join("sessions/2026/06/12");
        fs::create_dir_all(&session_dir).expect("create session dir");
        let session_path = session_dir.join("official-sim.jsonl");
        fs::write(
            &session_path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s1\",\"model_provider\":\"openai\"}}\n",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s2\",\"model_provider\":\"custom\"}}\n",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s3\",\"model_provider\":\"my-private-relay\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"text\":\"openai\"}}\n",
            ),
        )
        .expect("write session");

        let migrated_jsonl =
            migrate_codex_jsonl_files(&codex_dir, &source_provider_ids, &backup_root)
                .expect("migrate jsonl");
        assert_eq!(migrated_jsonl, 1);
        let session_text = fs::read_to_string(&session_path).expect("read session");
        assert_eq!(
            session_text
                .matches("\"model_provider\":\"custom\"")
                .count(),
            2
        );
        assert!(!session_text.contains("\"model_provider\":\"openai\""));
        assert!(session_text.contains("\"model_provider\":\"my-private-relay\""));
        assert!(
            session_text.contains("{\"type\":\"response_item\",\"payload\":{\"text\":\"openai\"}}")
        );
        assert!(backup_root
            .join("jsonl/sessions/2026/06/12/official-sim.jsonl")
            .exists());

        let rerun = migrate_codex_jsonl_files(&codex_dir, &source_provider_ids, &backup_root)
            .expect("rerun migrate jsonl");
        assert_eq!(rerun, 0);

        let state_db_path = codex_dir.join(CODEX_STATE_DB_FILENAME);
        let conn = Connection::open(&state_db_path).expect("open state db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                model_provider TEXT NOT NULL
            );
            INSERT INTO threads (id, model_provider) VALUES
                ('openai-thread', 'openai'),
                ('custom-thread', 'custom'),
                ('manual-thread', 'my-private-relay');",
        )
        .expect("seed state db");
        drop(conn);

        let migrated_state_rows = migrate_codex_state_db_provider_bucket(
            &state_db_path,
            &codex_dir,
            &source_provider_ids,
            &backup_root,
        )
        .expect("migrate state db");
        assert_eq!(migrated_state_rows, 1);

        let conn = Connection::open(&state_db_path).expect("reopen state db");
        let count_provider = |provider_id: &str| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM threads WHERE model_provider = ?1",
                [provider_id],
                |row| row.get(0),
            )
            .expect("count provider")
        };
        assert_eq!(count_provider("custom"), 2);
        assert_eq!(count_provider("openai"), 0);
        assert_eq!(count_provider("my-private-relay"), 1);
    }

    #[test]
    fn restores_only_ledgered_official_sessions_from_backups() {
        let dir = tempdir().expect("tempdir");
        let codex_dir = dir.path().join(".codex");
        let ledger_parent = dir.path().join("ledger");
        let restore_backup_root = dir.path().join("restore-backup");

        let generation = ledger_parent.join("20260612_010101");
        let backup_session_dir = generation.join("jsonl/sessions/2026/06/01");
        fs::create_dir_all(&backup_session_dir).expect("create backup session dir");
        fs::write(
            backup_session_dir.join("official.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s1\",\"model_provider\":\"openai\"}}\n",
        )
        .expect("write backup session");
        let backup_state_dir = generation.join("state");
        fs::create_dir_all(&backup_state_dir).expect("create backup state dir");
        let backup_db = Connection::open(backup_state_dir.join(CODEX_STATE_DB_FILENAME))
            .expect("open backup db");
        backup_db
            .execute_batch(
                "CREATE TABLE threads (id TEXT PRIMARY KEY, model_provider TEXT NOT NULL);
                INSERT INTO threads (id, model_provider) VALUES ('t1', 'openai');",
            )
            .expect("seed backup db");
        drop(backup_db);

        let session_dir = codex_dir.join("sessions/2026/06/01");
        fs::create_dir_all(&session_dir).expect("create session dir");
        let official_path = session_dir.join("official.jsonl");
        fs::write(
            &official_path,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s1\",\"model_provider\":\"custom\"}}\n",
        )
        .expect("write official session");
        let on_period_dir = codex_dir.join("sessions/2026/06/12");
        fs::create_dir_all(&on_period_dir).expect("create on-period dir");
        let on_period_path = on_period_dir.join("on-period.jsonl");
        fs::write(
            &on_period_path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s2\",\"model_provider\":\"custom\"}}\n",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s3\",\"model_provider\":\"my-private-relay\"}}\n",
            ),
        )
        .expect("write on-period session");

        let state_db_path = codex_dir.join(CODEX_STATE_DB_FILENAME);
        let conn = Connection::open(&state_db_path).expect("open state db");
        conn.execute_batch(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, model_provider TEXT NOT NULL);
            INSERT INTO threads (id, model_provider) VALUES
                ('t1', 'custom'),
                ('t2', 'custom'),
                ('t3', 'openai');",
        )
        .expect("seed state db");
        drop(conn);

        fs::write(
            generation.join("meta.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "codexConfigDir": canonical_dir_string(&codex_dir)
            }))
            .expect("serialize meta"),
        )
        .expect("write meta");

        let outcome = restore_codex_official_history_inner(
            &codex_dir,
            &ledger_parent,
            &restore_backup_root,
            "",
        )
        .expect("restore");
        assert_eq!(outcome.restored_jsonl_files, 1);
        assert_eq!(outcome.restored_state_rows, 1);
        assert!(outcome.skipped_reason.is_none());

        let official_text = fs::read_to_string(&official_path).expect("read official");
        assert!(official_text.contains("\"model_provider\":\"openai\""));
        let on_period_text = fs::read_to_string(&on_period_path).expect("read on-period");
        assert!(on_period_text.contains("\"id\":\"s2\",\"model_provider\":\"custom\""));
        assert!(on_period_text.contains("\"model_provider\":\"my-private-relay\""));

        let conn = Connection::open(&state_db_path).expect("reopen state db");
        let provider_of = |thread_id: &str| -> String {
            conn.query_row(
                "SELECT model_provider FROM threads WHERE id = ?1",
                [thread_id],
                |row| row.get(0),
            )
            .expect("thread provider")
        };
        assert_eq!(provider_of("t1"), "openai");
        assert_eq!(provider_of("t2"), "custom");
        assert_eq!(provider_of("t3"), "openai");
        drop(conn);

        assert!(restore_backup_root
            .join("jsonl/sessions/2026/06/01/official.jsonl")
            .exists());
        assert!(restore_backup_root
            .join("state")
            .join(CODEX_STATE_DB_FILENAME)
            .exists());

        let rerun = restore_codex_official_history_inner(
            &codex_dir,
            &ledger_parent,
            &dir.path().join("restore-backup-2"),
            "",
        )
        .expect("rerun restore");
        assert_eq!(rerun.restored_jsonl_files, 0);
        assert_eq!(rerun.restored_state_rows, 0);
        assert_eq!(rerun.skipped_reason.as_deref(), Some("nothing_to_restore"));
    }

    #[test]
    fn rewrites_only_codex_session_meta_provider_ids() {
        let dir = tempdir().expect("tempdir");
        let codex_dir = dir.path().join(".codex");
        let backup_root = dir.path().join("backup");
        let session_dir = codex_dir.join("sessions/2026/05/20");
        fs::create_dir_all(&session_dir).expect("create session dir");
        let path = session_dir.join("rollout-test.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s1\",\"model_provider\":\"rightcode\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"hi\"}}\n"
            ),
        )
        .expect("write session");

        let changed = rewrite_codex_session_file_for_provider_bucket(
            &path,
            &codex_dir,
            &HashSet::from(["rightcode".to_string()]),
            &backup_root,
        )
        .expect("rewrite");

        assert!(changed);
        let next = fs::read_to_string(&path).expect("read rewritten");
        assert!(next.contains("\"model_provider\":\"custom\""));
        assert!(backup_root
            .join("jsonl/sessions/2026/05/20/rollout-test.jsonl")
            .exists());
    }

    #[test]
    fn does_not_rewrite_unknown_jsonl_history_without_trusted_source_id() {
        let dir = tempdir().expect("tempdir");
        let codex_dir = dir.path().join(".codex");
        let session_dir = codex_dir.join("sessions/2026/05/20");
        fs::create_dir_all(&session_dir).expect("create session dir");
        let path = session_dir.join("rollout-rightcode.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s1\",\"model_provider\":\"rightcode\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"hi\"}}\n"
            ),
        )
        .expect("write session");

        let backup_root = dir.path().join("backup");
        let changed = migrate_codex_jsonl_files(
            &codex_dir,
            &source_ids(&["some-trusted-provider"]),
            &backup_root,
        )
        .expect("migrate jsonl");

        assert_eq!(changed, 0);
        let next = fs::read_to_string(&path).expect("read session");
        assert!(next.contains("\"model_provider\":\"rightcode\""));
        assert!(!backup_root.exists());
    }

    #[test]
    fn does_not_update_unknown_state_db_history_without_trusted_source_id() {
        let dir = tempdir().expect("tempdir");
        let codex_dir = dir.path().join(".codex");
        fs::create_dir_all(&codex_dir).expect("create codex dir");
        let db_path = codex_dir.join(CODEX_STATE_DB_FILENAME);
        let conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                model_provider TEXT NOT NULL
            );
            INSERT INTO threads (id, model_provider) VALUES
                ('a', 'aihubmix'),
                ('b', 'openai'),
                ('c', 'custom');",
        )
        .expect("seed db");
        drop(conn);

        let backup_root = dir.path().join("backup");
        let changed = migrate_codex_state_db_provider_bucket(
            &db_path,
            &codex_dir,
            &source_ids(&["rightcode"]),
            &backup_root,
        )
        .expect("migrate state db");

        assert_eq!(changed, 0);
        let conn = Connection::open(&db_path).expect("reopen db");
        let aihubmix_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM threads WHERE model_provider = 'aihubmix'",
                [],
                |row| row.get(0),
            )
            .expect("count aihubmix");
        assert_eq!(aihubmix_count, 1);
        assert!(!backup_root.exists());
    }

    #[test]
    fn updates_codex_state_db_thread_provider_ids() {
        let dir = tempdir().expect("tempdir");
        let codex_dir = dir.path().join(".codex");
        fs::create_dir_all(&codex_dir).expect("create codex dir");
        let db_path = codex_dir.join(CODEX_STATE_DB_FILENAME);
        let conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                model_provider TEXT NOT NULL
            );
            INSERT INTO threads (id, model_provider) VALUES
                ('a', 'rightcode'),
                ('b', 'openai'),
                ('c', 'aihubmix');",
        )
        .expect("seed db");
        drop(conn);

        let backup_root = dir.path().join("backup");
        let changed = migrate_codex_state_db_provider_bucket(
            &db_path,
            &codex_dir,
            &source_ids(&["rightcode", "aihubmix"]),
            &backup_root,
        )
        .expect("migrate state db");

        assert_eq!(changed, 2);
        let conn = Connection::open(&db_path).expect("reopen db");
        let custom_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM threads WHERE model_provider = 'custom'",
                [],
                |row| row.get(0),
            )
            .expect("count custom");
        let openai_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM threads WHERE model_provider = 'openai'",
                [],
                |row| row.get(0),
            )
            .expect("count openai");
        assert_eq!(custom_count, 2);
        assert_eq!(openai_count, 1);

        let backup_path = backup_root.join("state").join(CODEX_STATE_DB_FILENAME);
        let backup_conn = Connection::open(&backup_path).expect("open backup db");
        let backed_up_source_count: i64 = backup_conn
            .query_row(
                "SELECT COUNT(*) FROM threads WHERE model_provider IN ('rightcode', 'aihubmix')",
                [],
                |row| row.get(0),
            )
            .expect("count backed up source providers");
        assert_eq!(backed_up_source_count, 2);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&backup_path)
                .expect("metadata backup db")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn codex_state_db_backup_does_not_throttle_completed_pages() {
        let dir = tempdir().expect("tempdir");
        let codex_dir = dir.path().join(".codex");
        fs::create_dir_all(&codex_dir).expect("create codex dir");
        let db_path = codex_dir.join(CODEX_STATE_DB_FILENAME);
        let conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                model_provider TEXT NOT NULL
            );
            CREATE TABLE backup_padding (payload BLOB NOT NULL);
            INSERT INTO threads (id, model_provider) VALUES ('a', 'rightcode');
            INSERT INTO backup_padding (payload) VALUES (zeroblob(8388608));",
        )
        .expect("seed large state db");

        let backup_root = dir.path().join("backup");
        let started = std::time::Instant::now();
        backup_codex_state_db(&db_path, &codex_dir, &backup_root, &conn)
            .expect("back up large Codex state db");
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(5),
            "an 8 MiB backup must not sleep after every five pages: {elapsed:?}"
        );
        let backup_path = backup_root.join("state").join(CODEX_STATE_DB_FILENAME);
        let backup_conn = Connection::open(&backup_path).expect("open backup db");
        let integrity: String = backup_conn
            .query_row("PRAGMA integrity_check;", [], |row| row.get(0))
            .expect("check backup integrity");
        assert_eq!(integrity, "ok");
        let bytes: i64 = backup_conn
            .query_row("SELECT length(payload) FROM backup_padding", [], |row| {
                row.get(0)
            })
            .expect("read backed up payload");
        assert_eq!(bytes, 8 * 1024 * 1024);
    }

    #[test]
    fn codex_state_db_backup_cleans_files_after_a_real_busy_timeout() {
        let dir = tempdir().expect("tempdir");
        let codex_dir = dir.path().join(".codex");
        fs::create_dir_all(&codex_dir).expect("create codex dir");
        let db_path = codex_dir.join(CODEX_STATE_DB_FILENAME);
        let conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            "PRAGMA journal_mode=DELETE;
             CREATE TABLE threads (
                 id TEXT PRIMARY KEY,
                 model_provider TEXT NOT NULL
             );
             INSERT INTO threads (id, model_provider) VALUES ('a', 'rightcode');",
        )
        .expect("seed state db");
        conn.busy_timeout(Duration::from_millis(50))
            .expect("set source busy timeout");

        let locker = Connection::open(&db_path).expect("open lock connection");
        locker
            .execute_batch("BEGIN EXCLUSIVE;")
            .expect("lock source db");

        let backup_root = dir.path().join("backup");
        let started = std::time::Instant::now();
        let error = backup_codex_state_db(&db_path, &codex_dir, &backup_root, &conn)
            .expect_err("locked source backup should fail");
        let elapsed = started.elapsed();
        locker
            .execute_batch("ROLLBACK;")
            .expect("release source db");

        assert!(
            elapsed < Duration::from_secs(1),
            "the backup layer must not multiply the 50ms connection timeout: {elapsed:?}"
        );
        assert!(error.to_string().contains("busy_timeout"));

        let backup_path = backup_root.join("state").join(CODEX_STATE_DB_FILENAME);
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let mut artifact = backup_path.as_os_str().to_os_string();
            artifact.push(suffix);
            assert!(
                !PathBuf::from(artifact).exists(),
                "failed Codex backup must not leave {suffix} artifact"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn codex_migration_backups_reject_parent_dir_config_path_before_creating_dirs() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("config-root");
        fs::create_dir(&root).expect("create root");
        let backup_root = root.join("child").join("..").join("backups");
        let _env = crate::test_support::TestEnvGuard::isolated(dir.path());
        unsafe {
            std::env::set_var("CC_SWITCH_CONFIG_DIR", root.join("child").join(".."));
        }

        let codex_dir = dir.path().join(".codex");
        let session_dir = codex_dir.join("sessions");
        fs::create_dir_all(&session_dir).expect("create session dir");
        let jsonl = session_dir.join("session.jsonl");
        fs::write(&jsonl, "{}\n").expect("write jsonl");
        copy_existing_file(&jsonl, &backup_root.join("jsonl").join("session.jsonl"))
            .expect_err("jsonl backup should reject invalid config dir");

        backup_provider_settings_config(
            "provider",
            &serde_json::json!({ "config": "model_provider = \"custom\"" }),
            &backup_root,
        )
        .expect_err("provider backup should reject invalid config dir");

        let state_db = codex_dir.join(CODEX_STATE_DB_FILENAME);
        let conn = Connection::open(&state_db).expect("open state db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                model_provider TEXT NOT NULL
            );",
        )
        .expect("seed state db");
        backup_codex_state_db(&state_db, &codex_dir, &backup_root, &conn)
            .expect_err("state db backup should reject invalid config dir");

        assert!(
            !root.join("child").exists(),
            "backup helpers must not pre-create unvalidated path components"
        );
        assert!(
            !root.join("backups").exists(),
            "backup helpers must not write to the normalized parent directory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn codex_state_db_backup_rejects_symlink_backup_path_without_writing_target() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let _env = crate::test_support::TestEnvGuard::isolated(dir.path());

        let codex_dir = dir.path().join(".codex");
        fs::create_dir_all(&codex_dir).expect("create codex dir");
        let state_db = codex_dir.join(CODEX_STATE_DB_FILENAME);
        let conn = Connection::open(&state_db).expect("open state db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                model_provider TEXT NOT NULL
            );",
        )
        .expect("seed state db");

        let backup_root = get_app_config_dir().join("backups").join("migration");
        let backup_path = backup_root.join("state").join(CODEX_STATE_DB_FILENAME);
        fs::create_dir_all(backup_path.parent().expect("backup parent"))
            .expect("create backup parent");
        let external_target = dir.path().join("external-state.sqlite");
        symlink(&external_target, &backup_path).expect("create dangling backup symlink");

        let err = backup_codex_state_db(&state_db, &codex_dir, &backup_root, &conn)
            .expect_err("state db backup must reject symlink backup path");

        assert!(
            err.to_string().contains("符号链接") || err.to_string().contains("symlink"),
            "unexpected error: {err}"
        );
        assert!(
            !external_target.exists(),
            "state db backup must not follow symlink target"
        );
    }

    #[test]
    fn codex_state_db_backup_rejects_existing_backup_path() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().expect("tempdir");
        let _env = crate::test_support::TestEnvGuard::isolated(dir.path());

        let codex_dir = dir.path().join(".codex");
        fs::create_dir_all(&codex_dir).expect("create codex dir");
        let state_db = codex_dir.join(CODEX_STATE_DB_FILENAME);
        let conn = Connection::open(&state_db).expect("open state db");
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                model_provider TEXT NOT NULL
            );",
        )
        .expect("seed state db");

        let backup_root = get_app_config_dir().join("backups").join("migration");
        let backup_path = backup_root.join("state").join(CODEX_STATE_DB_FILENAME);
        fs::create_dir_all(backup_path.parent().expect("backup parent"))
            .expect("create backup parent");
        fs::write(&backup_path, b"existing").expect("write existing backup");
        #[cfg(unix)]
        fs::set_permissions(&backup_path, fs::Permissions::from_mode(0o644))
            .expect("set existing backup permissions");

        let err = backup_codex_state_db(&state_db, &codex_dir, &backup_root, &conn)
            .expect_err("existing state db backup path must be rejected");

        assert!(
            err.to_string().contains("已存在") || err.to_string().contains("exists"),
            "unexpected error: {err}"
        );
        assert_eq!(
            fs::read(&backup_path).expect("read existing backup"),
            b"existing",
            "state db backup must not overwrite an existing backup file"
        );
        #[cfg(unix)]
        {
            let mode = fs::metadata(&backup_path)
                .expect("metadata existing backup")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                mode, 0o644,
                "rejected existing backup path should be left untouched"
            );
        }
    }

    #[test]
    fn collects_third_party_provider_ids_from_codex_providers() {
        let db = Database::memory().expect("memory db");
        let third_party = Provider::with_id(
            "rightcode".to_string(),
            "RightCode".to_string(),
            serde_json::json!({
                "auth": {},
                "config": "model_provider = \"aihubmix\"\n\n[model_providers.aihubmix]\nname = \"AIHubMix\"\nbase_url = \"https://example.com/v1\""
            }),
            None,
        );
        let mut official = Provider::with_id(
            "codex-official".to_string(),
            "OpenAI Official".to_string(),
            serde_json::json!({"auth": {}, "config": "model_provider = \"openai\""}),
            None,
        );
        official.category = Some("official".to_string());

        db.save_provider("codex", &third_party)
            .expect("save third-party");
        db.save_provider("codex", &official).expect("save official");

        let ids = collect_source_model_provider_ids(&db).expect("collect ids");
        assert!(ids.contains("rightcode"));
        assert!(ids.contains("aihubmix"));
        assert!(!ids.contains("openai"));
        assert!(!ids.contains("codex-official"));
    }

    #[test]
    fn migrates_cli_generated_dynamic_provider_bucket_end_to_end() {
        let dir = tempdir().expect("tempdir");
        let codex_dir = dir.path().join(".codex");
        let backup_root = dir.path().join("backup");
        let session_dir = codex_dir.join("sessions/2026/07/16");
        fs::create_dir_all(&session_dir).expect("create session dir");

        let db = Database::memory().expect("memory db");
        let mut generated = Provider::with_id(
            "my-relay".to_string(),
            "My Relay".to_string(),
            serde_json::json!({
                "auth": {},
                "config": r#"model_provider = "my_relay"
model = "gpt-5.4"
model_reasoning_effort = "high"
disable_response_storage = true
profile = "work"

[model_providers.my_relay]
name = "my_relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true

[profiles.work]
model_provider = "my_relay"
model = "gpt-5.4"
"#
            }),
            None,
        );
        generated.category = Some("custom".to_string());
        generated.created_at = Some(1);
        db.save_provider("codex", &generated)
            .expect("save generated provider");

        let deeplink = Provider::with_id(
            "deeplink-1784160000001".to_string(),
            "Renamed Deep Link".to_string(),
            serde_json::json!({
                "auth": { "OPENAI_API_KEY": "sk-deeplink" },
                "config": r#"model_provider = "deep_link"
model = "gpt-5.4"

[model_providers.deep_link]
base_url = "https://deeplink.example/v1"
wire_api = "responses"
requires_openai_auth = false
env_key = "OPENAI_API_KEY"
"#
            }),
            None,
        );
        db.save_provider("codex", &deeplink)
            .expect("save legacy deeplink provider");

        let full_deeplink = Provider::with_id(
            "full-link-1784160000051".to_string(),
            "Renamed Full Deep Link".to_string(),
            serde_json::json!({
                "auth": { "OPENAI_API_KEY": "sk-full" },
                "config": r#"model_provider = "full_link"
model = "gpt-5.4"
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.full_link]
name = "full_link"
base_url = "https://full.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
            }),
            None,
        );
        db.save_provider("codex", &full_deeplink)
            .expect("save full legacy deeplink provider");

        let converted_flat_deeplink = Provider::with_id(
            "legacyflat-1784160000101".to_string(),
            "Renamed Legacy Flat".to_string(),
            serde_json::json!({
                "auth": { "OPENAI_API_KEY": "sk-flat" },
                "config": r#"model_provider = "legacyflat_1784160000101"
model = "gpt-5.4"

[model_providers.legacyflat_1784160000101]
name = "legacyflat_1784160000101"
base_url = "https://flat.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
            }),
            None,
        );
        db.save_provider("codex", &converted_flat_deeplink)
            .expect("save converted flat deeplink provider");

        let mut manual = Provider::with_id(
            "manual-relay".to_string(),
            "Manual Relay".to_string(),
            serde_json::json!({
                "auth": {},
                "config": r#"model_provider = "manual_relay"
model = "gpt-5.4"

[model_providers.manual_relay]
name = "manual_relay"
base_url = "https://manual.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
            }),
            None,
        );
        manual.category = Some("custom".to_string());
        manual.created_at = Some(1);
        db.save_provider("codex", &manual)
            .expect("save manual provider");

        let source_provider_ids = collect_source_model_provider_ids(&db).expect("collect ids");
        assert_eq!(
            source_provider_ids,
            source_ids(&[
                "deep_link",
                "full_link",
                "legacyflat_1784160000101",
                "my_relay",
            ])
        );

        let session_path = session_dir.join("dynamic-provider.jsonl");
        fs::write(
            &session_path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"generated\",\"model_provider\":\"my_relay\"}}\n",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"deeplink\",\"model_provider\":\"deep_link\"}}\n",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"full\",\"model_provider\":\"full_link\"}}\n",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"flat\",\"model_provider\":\"legacyflat_1784160000101\"}}\n",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"manual\",\"model_provider\":\"manual_relay\"}}\n",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"official\",\"model_provider\":\"openai\"}}\n",
            ),
        )
        .expect("write session file");

        let state_db = codex_dir.join(CODEX_STATE_DB_FILENAME);
        let conn = Connection::open(&state_db).expect("open state db");
        conn.execute_batch(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, model_provider TEXT NOT NULL);\
             INSERT INTO threads VALUES ('generated', 'my_relay');\
             INSERT INTO threads VALUES ('deeplink', 'deep_link');\
             INSERT INTO threads VALUES ('full', 'full_link');\
             INSERT INTO threads VALUES ('flat', 'legacyflat_1784160000101');\
             INSERT INTO threads VALUES ('manual', 'manual_relay');\
             INSERT INTO threads VALUES ('official', 'openai');",
        )
        .expect("seed state db");
        drop(conn);

        assert_eq!(
            migrate_codex_jsonl_files(&codex_dir, &source_provider_ids, &backup_root)
                .expect("migrate session files"),
            1
        );
        assert_eq!(
            migrate_codex_state_db_provider_bucket(
                &state_db,
                &codex_dir,
                &source_provider_ids,
                &backup_root,
            )
            .expect("migrate state db"),
            4
        );
        let mut template_outcome = migrate_codex_provider_templates_to_custom(&db, &backup_root)
            .expect("migrate provider templates");
        template_outcome.migrated_provider_ids.sort();
        assert_eq!(
            template_outcome.migrated_provider_ids,
            vec![
                "deeplink-1784160000001".to_string(),
                "full-link-1784160000051".to_string(),
                "legacyflat-1784160000101".to_string(),
                "my-relay".to_string()
            ]
        );

        let session_text = fs::read_to_string(&session_path).expect("read migrated session");
        assert!(session_text.contains("\"id\":\"generated\",\"model_provider\":\"custom\""));
        assert!(session_text.contains("\"id\":\"deeplink\",\"model_provider\":\"custom\""));
        assert!(session_text.contains("\"id\":\"full\",\"model_provider\":\"custom\""));
        assert!(session_text.contains("\"id\":\"flat\",\"model_provider\":\"custom\""));
        assert!(session_text.contains("\"id\":\"manual\",\"model_provider\":\"manual_relay\""));
        assert!(session_text.contains("\"id\":\"official\",\"model_provider\":\"openai\""));

        let conn = Connection::open(&state_db).expect("reopen state db");
        for (id, expected_provider) in [
            ("generated", "custom"),
            ("deeplink", "custom"),
            ("full", "custom"),
            ("flat", "custom"),
            ("manual", "manual_relay"),
            ("official", "openai"),
        ] {
            let provider: String = conn
                .query_row(
                    "SELECT model_provider FROM threads WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .expect("read thread provider");
            assert_eq!(provider, expected_provider);
        }
        drop(conn);

        let migrated = db
            .get_provider_by_id("my-relay", "codex")
            .expect("get generated provider")
            .expect("generated provider exists");
        let migrated_config = migrated
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .expect("generated config");
        let migrated_doc: toml::Value =
            toml::from_str(migrated_config).expect("parse generated config");
        assert_eq!(
            migrated_doc
                .get("model_provider")
                .and_then(toml::Value::as_str),
            Some("custom")
        );

        let migrated_deeplink = db
            .get_provider_by_id("deeplink-1784160000001", "codex")
            .expect("get deeplink provider")
            .expect("deeplink provider exists");
        let migrated_deeplink_doc: toml::Value = toml::from_str(
            migrated_deeplink
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .expect("deeplink config"),
        )
        .expect("parse migrated deeplink config");
        assert_eq!(
            migrated_deeplink_doc
                .get("model_provider")
                .and_then(toml::Value::as_str),
            Some("custom")
        );
        assert_eq!(
            migrated_deeplink_doc
                .get("model_providers")
                .and_then(|providers| providers.get("custom"))
                .and_then(|provider| provider.get("name"))
                .and_then(toml::Value::as_str),
            Some("Renamed Deep Link")
        );

        let migrated_full = db
            .get_provider_by_id("full-link-1784160000051", "codex")
            .expect("get full deeplink provider")
            .expect("full deeplink provider exists");
        let migrated_full_doc: toml::Value = toml::from_str(
            migrated_full
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .expect("full deeplink config"),
        )
        .expect("parse migrated full deeplink config");
        assert_eq!(
            migrated_full_doc
                .get("model_provider")
                .and_then(toml::Value::as_str),
            Some("custom")
        );
        assert_eq!(
            migrated_full_doc
                .get("model_providers")
                .and_then(|providers| providers.get("custom"))
                .and_then(|provider| provider.get("name"))
                .and_then(toml::Value::as_str),
            Some("full_link")
        );

        let migrated_flat = db
            .get_provider_by_id("legacyflat-1784160000101", "codex")
            .expect("get converted flat provider")
            .expect("converted flat provider exists");
        let migrated_flat_doc: toml::Value = toml::from_str(
            migrated_flat
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .expect("converted flat config"),
        )
        .expect("parse migrated converted flat config");
        assert_eq!(
            migrated_flat_doc
                .get("model_provider")
                .and_then(toml::Value::as_str),
            Some("custom")
        );
        assert_eq!(
            migrated_flat_doc
                .get("model_providers")
                .and_then(|providers| providers.get("custom"))
                .and_then(|provider| provider.get("name"))
                .and_then(toml::Value::as_str),
            Some("legacyflat_1784160000101")
        );
        assert!(migrated_doc
            .get("model_providers")
            .and_then(|providers| providers.get("my_relay"))
            .is_none());
        assert_eq!(
            migrated_doc
                .get("profiles")
                .and_then(|profiles| profiles.get("work"))
                .and_then(|profile| profile.get("model_provider"))
                .and_then(toml::Value::as_str),
            Some("custom")
        );

        let manual = db
            .get_provider_by_id("manual-relay", "codex")
            .expect("get manual provider")
            .expect("manual provider exists");
        assert!(manual
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .is_some_and(|config| config.contains("model_provider = \"manual_relay\"")));

        assert!(backup_root.join("providers").is_dir());
        assert!(backup_root
            .join("jsonl/sessions/2026/07/16/dynamic-provider.jsonl")
            .is_file());
        assert!(backup_root
            .join("state")
            .join(CODEX_STATE_DB_FILENAME)
            .is_file());

        assert_eq!(
            migrate_codex_jsonl_files(&codex_dir, &source_provider_ids, &backup_root)
                .expect("repeat session migration"),
            0
        );
        assert_eq!(
            migrate_codex_state_db_provider_bucket(
                &state_db,
                &codex_dir,
                &source_provider_ids,
                &backup_root,
            )
            .expect("repeat state migration"),
            0
        );
        assert!(
            migrate_codex_provider_templates_to_custom(&db, &backup_root)
                .expect("repeat template migration")
                .migrated_provider_ids
                .is_empty()
        );
    }

    #[test]
    fn imported_default_provider_does_not_guess_dynamic_bucket_from_config() {
        let config = r#"model_provider = "imported_relay"
model = "gpt-5.4"
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.imported_relay]
name = "imported_relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#;
        let provider = Provider::with_id(
            "default".to_string(),
            "Imported Configuration".to_string(),
            serde_json::json!({ "auth": {}, "config": config }),
            None,
        );
        assert_eq!(
            cli_generated_quick_config_model_provider_id(&provider, config),
            None
        );
    }

    #[test]
    fn legacy_deeplink_detector_requires_timestamped_id_name_fingerprint() {
        let config = r#"model_provider = "deep_link"
model = "gpt-5.4"

[model_providers.deep_link]
base_url = "https://deeplink.example/v1"
wire_api = "responses"
requires_openai_auth = false
env_key = "OPENAI_API_KEY"
"#;
        let mut provider = Provider::with_id(
            "unrelated-1784160000001".to_string(),
            "Renamed Deep Link".to_string(),
            serde_json::json!({
                "auth": { "OPENAI_API_KEY": "sk-deeplink" },
                "config": config
            }),
            None,
        );
        provider.created_at = Some(1_784_160_000_000);

        assert_eq!(
            legacy_cli_deeplink_model_provider_id(&provider, config),
            None
        );
    }

    #[test]
    fn legacy_deeplink_detector_rejects_conflicting_created_at() {
        let config = r#"model_provider = "deep_link"
model = "gpt-5.4"

[model_providers.deep_link]
base_url = "https://deeplink.example/v1"
wire_api = "responses"
requires_openai_auth = false
env_key = "OPENAI_API_KEY"
"#;
        let mut provider = Provider::with_id(
            "deeplink-1784160000001".to_string(),
            "Deep Link".to_string(),
            serde_json::json!({
                "auth": { "OPENAI_API_KEY": "sk-deeplink" },
                "config": config
            }),
            None,
        );
        provider.created_at = Some(1_700_000_000_000);

        assert_eq!(
            legacy_cli_deeplink_model_provider_id(&provider, config),
            None
        );
    }

    #[test]
    fn dynamic_detector_uses_effective_common_config_without_copying_it_to_storage() {
        let db = Database::memory().expect("memory db");
        db.set_config_snippet(
            "codex",
            Some(
                "model_reasoning_effort = \"high\"\ndisable_response_storage = true\n".to_string(),
            ),
        )
        .expect("save common config");
        let mut provider = Provider::with_id(
            "common-relay".to_string(),
            "Common Relay".to_string(),
            serde_json::json!({
                "auth": {},
                "config": r#"model_provider = "common_relay"
model = "gpt-5.4"

[model_providers.common_relay]
name = "common_relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
            }),
            None,
        );
        provider
            .meta
            .get_or_insert_with(crate::provider::ProviderMeta::default)
            .apply_common_config = Some(true);
        db.save_provider("codex", &provider).expect("save provider");

        let source_ids = collect_source_model_provider_ids(&db).expect("collect ids");
        assert!(source_ids.contains("common_relay"));
        let backup_dir = tempdir().expect("backup dir");
        let outcome = migrate_codex_provider_templates_to_custom(&db, backup_dir.path())
            .expect("migrate templates");
        assert_eq!(outcome.migrated_provider_ids, vec!["common-relay"]);

        let saved = db
            .get_provider_by_id("common-relay", "codex")
            .expect("get provider")
            .expect("provider exists");
        let config = saved
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .expect("stored config");
        assert!(config.contains("model_provider = \"custom\""));
        assert!(!config.contains("model_reasoning_effort"));
        assert!(!config.contains("disable_response_storage"));
    }

    #[test]
    fn full_deeplink_detector_uses_effective_common_config_after_storage_normalization() {
        let db = Database::memory().expect("memory db");
        db.set_config_snippet(
            "codex",
            Some(
                "model_reasoning_effort = \"high\"\ndisable_response_storage = true\n".to_string(),
            ),
        )
        .expect("save common config");
        let mut provider = Provider::with_id(
            "full-link-1784160000051".to_string(),
            "Renamed Full Deep Link".to_string(),
            serde_json::json!({
                "auth": { "OPENAI_API_KEY": "sk-full" },
                "config": r#"model_provider = "full_link"
model = "gpt-5.4"

[model_providers.full_link]
name = "full_link"
base_url = "https://full.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
            }),
            None,
        );
        provider
            .meta
            .get_or_insert_with(crate::provider::ProviderMeta::default)
            .apply_common_config = Some(true);
        db.save_provider("codex", &provider).expect("save provider");

        let source_ids = collect_source_model_provider_ids(&db).expect("collect ids");
        assert!(source_ids.contains("full_link"));
    }

    #[test]
    fn quick_detector_uses_raw_config_when_common_config_overrides_generated_defaults() {
        let db = Database::memory().expect("memory db");
        db.set_config_snippet(
            "codex",
            Some(
                "model_reasoning_effort = \"medium\"\ndisable_response_storage = false\n"
                    .to_string(),
            ),
        )
        .expect("save common config");
        let mut provider = Provider::with_id(
            "common-relay".to_string(),
            "Common Relay".to_string(),
            serde_json::json!({
                "auth": {},
                "config": r#"model_provider = "common_relay"
model = "gpt-5.4"
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.common_relay]
name = "common_relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
            }),
            None,
        );
        provider
            .meta
            .get_or_insert_with(crate::provider::ProviderMeta::default)
            .apply_common_config = Some(true);
        db.save_provider("codex", &provider).expect("save provider");

        let source_ids = collect_source_model_provider_ids(&db).expect("collect ids");
        assert!(source_ids.contains("common_relay"));
    }

    #[test]
    fn skips_unknown_provider_model_provider_id_from_existing_config() {
        let db = Database::memory().expect("memory db");
        let mut provider = Provider::with_id(
            "manual-aggregator".to_string(),
            "Manual Aggregator".to_string(),
            serde_json::json!({
                "auth": {},
                "config": "model_provider = \"my-private-relay\"\n\n[model_providers.my-private-relay]\nname = \"Manual Relay\"\nbase_url = \"http://localhost:8080/v1\""
            }),
            None,
        );
        provider.category = Some("aggregator".to_string());

        db.save_provider("codex", &provider).expect("save provider");

        let ids = collect_source_model_provider_ids(&db).expect("collect ids");
        assert!(!ids.contains("my-private-relay"));
    }

    #[test]
    fn skips_undefined_provider_model_provider_id_from_existing_config() {
        let db = Database::memory().expect("memory db");
        let mut provider = Provider::with_id(
            "manual-aggregator".to_string(),
            "Manual Aggregator".to_string(),
            serde_json::json!({
                "auth": {},
                "config": "model_provider = \"my-private-relay\"\n"
            }),
            None,
        );
        provider.category = Some("aggregator".to_string());

        db.save_provider("codex", &provider).expect("save provider");

        let ids = collect_source_model_provider_ids(&db).expect("collect ids");
        assert!(!ids.contains("my-private-relay"));
    }

    #[test]
    fn skips_unknown_profile_model_provider_id_from_existing_config() {
        let db = Database::memory().expect("memory db");
        let mut provider = Provider::with_id(
            "manual-aggregator".to_string(),
            "Manual Aggregator".to_string(),
            serde_json::json!({
                "auth": {},
                "config": r#"profile = "work"

[model_providers.my-private-relay]
name = "Manual Relay"
base_url = "http://localhost:8080/v1"

[profiles.work]
model_provider = "my-private-relay"
"#
            }),
            None,
        );
        provider.category = Some("aggregator".to_string());

        db.save_provider("codex", &provider).expect("save provider");

        let ids = collect_source_model_provider_ids(&db).expect("collect ids");
        assert!(!ids.contains("my-private-relay"));
    }

    #[test]
    fn collects_known_legacy_provider_id_from_normalized_preset_config() {
        let db = Database::memory().expect("memory db");
        let mut provider = Provider::with_id(
            "generated-uuid".to_string(),
            "AIHubMix".to_string(),
            serde_json::json!({
                "auth": {},
                "config": "model_provider = \"custom\"\n\n[model_providers.custom]\nname = \"AIHubMix\"\nbase_url = \"https://aihubmix.example/v1\""
            }),
            None,
        );
        provider.category = Some("aggregator".to_string());

        db.save_provider("codex", &provider).expect("save provider");

        let ids = collect_source_model_provider_ids(&db).expect("collect ids");
        assert!(ids.contains("aihubmix"));
        assert!(!ids.contains("generated-uuid"));
    }

    #[test]
    fn recognizes_upstream_etok_legacy_provider_id() {
        assert!(is_known_cc_switch_legacy_codex_model_provider_id("etok"));
    }

    #[test]
    fn collects_legacy_ccswitch_provider_id_from_stored_config() {
        let db = Database::memory().expect("memory db");
        let mut provider = Provider::with_id(
            "generated-uuid".to_string(),
            "Legacy Stable".to_string(),
            serde_json::json!({
                "auth": {},
                "config": "model_provider = \"ccswitch\"\n\n[model_providers.ccswitch]\nname = \"AIHubMix\"\nbase_url = \"https://aihubmix.example/v1\""
            }),
            None,
        );
        provider.category = Some("aggregator".to_string());

        db.save_provider("codex", &provider).expect("save provider");

        let ids = collect_source_model_provider_ids(&db).expect("collect ids");
        assert!(ids.contains("ccswitch"));
        assert!(ids.contains("aihubmix"));
        assert!(!ids.contains("generated-uuid"));
    }

    #[test]
    fn migrates_stored_provider_template_to_custom() {
        let db = Database::memory().expect("memory db");
        let provider = Provider::with_id(
            "legacy".to_string(),
            "Legacy Stable".to_string(),
            serde_json::json!({
                "auth": {},
                "config": r#"model_provider = "aihubmix"
model = "gpt-5.4"
profile = "work"

[model_providers.aihubmix]
name = "AIHubMix"
base_url = "https://aihubmix.example/v1"
wire_api = "responses"

[profiles.work]
model_provider = "aihubmix"
model = "gpt-5.4"
"#
            }),
            None,
        );
        db.save_provider("codex", &provider).expect("save provider");

        let (outcome, backup_dir) = migrate_provider_templates_for_test(&db);
        assert_eq!(outcome.migrated_provider_ids, vec!["legacy".to_string()]);

        let saved = db
            .get_provider_by_id("legacy", "codex")
            .expect("get provider")
            .expect("provider exists");
        let config_text = saved
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .expect("config text");
        let parsed: toml::Value = toml::from_str(config_text).expect("parse config");

        assert_eq!(
            parsed
                .get("model_provider")
                .and_then(|value| value.as_str()),
            Some("custom")
        );
        assert!(parsed
            .get("model_providers")
            .and_then(|value| value.get("aihubmix"))
            .is_none());
        assert_eq!(
            parsed
                .get("model_providers")
                .and_then(|value| value.get("custom"))
                .and_then(|value| value.get("base_url"))
                .and_then(|value| value.as_str()),
            Some("https://aihubmix.example/v1")
        );
        assert_eq!(
            parsed
                .get("profiles")
                .and_then(|value| value.get("work"))
                .and_then(|value| value.get("model_provider"))
                .and_then(|value| value.as_str()),
            Some("custom")
        );

        let backups: Vec<_> = fs::read_dir(backup_dir.path().join("providers"))
            .expect("provider backups")
            .flatten()
            .collect();
        assert_eq!(backups.len(), 1);
        let backup_text = fs::read_to_string(backups[0].path()).expect("read provider backup");
        assert!(backup_text.contains(r#""providerId": "legacy""#));
        assert!(backup_text.contains(r#"model_provider = \"aihubmix\""#));

        let (second, _second_backup_dir) = migrate_provider_templates_for_test(&db);
        assert!(second.migrated_provider_ids.is_empty());
    }

    #[test]
    fn migrates_legacy_ccswitch_provider_template_to_custom() {
        let db = Database::memory().expect("memory db");
        let provider = Provider::with_id(
            "legacy-ccswitch".to_string(),
            "Legacy CC Switch".to_string(),
            serde_json::json!({
                "auth": {},
                "config": r#"model_provider = "ccswitch"

[model_providers.ccswitch]
name = "AIHubMix"
base_url = "https://aihubmix.example/v1"
"#
            }),
            None,
        );
        db.save_provider("codex", &provider).expect("save provider");

        let (outcome, _backup_dir) = migrate_provider_templates_for_test(&db);
        assert_eq!(
            outcome.migrated_provider_ids,
            vec!["legacy-ccswitch".to_string()]
        );

        let saved = db
            .get_provider_by_id("legacy-ccswitch", "codex")
            .expect("get provider")
            .expect("provider exists");
        let config_text = saved
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .expect("config text");
        let parsed: toml::Value = toml::from_str(config_text).expect("parse config");

        assert_eq!(
            parsed
                .get("model_provider")
                .and_then(|value| value.as_str()),
            Some("custom")
        );
        assert!(parsed
            .get("model_providers")
            .and_then(|value| value.get("ccswitch"))
            .is_none());
        assert_eq!(
            parsed
                .get("model_providers")
                .and_then(|value| value.get("custom"))
                .and_then(|value| value.get("base_url"))
                .and_then(|value| value.as_str()),
            Some("https://aihubmix.example/v1")
        );
    }

    #[test]
    fn skips_unknown_stored_provider_template() {
        let db = Database::memory().expect("memory db");
        let provider = Provider::with_id(
            "manual".to_string(),
            "Manual Relay".to_string(),
            serde_json::json!({
                "auth": {},
                "config": r#"model_provider = "my-private-relay"

[model_providers.my-private-relay]
name = "Manual Relay"
base_url = "http://localhost:8080/v1"
"#
            }),
            None,
        );
        db.save_provider("codex", &provider).expect("save provider");

        let (outcome, _backup_dir) = migrate_provider_templates_for_test(&db);
        assert!(outcome.migrated_provider_ids.is_empty());

        let saved = db
            .get_provider_by_id("manual", "codex")
            .expect("get provider")
            .expect("provider exists");
        let config_text = saved
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .expect("config text");
        let parsed: toml::Value = toml::from_str(config_text).expect("parse config");

        assert_eq!(
            parsed
                .get("model_provider")
                .and_then(|value| value.as_str()),
            Some("my-private-relay")
        );
        assert_eq!(
            parsed
                .get("model_providers")
                .and_then(|value| value.get("my-private-relay"))
                .and_then(|value| value.get("base_url"))
                .and_then(|value| value.as_str()),
            Some("http://localhost:8080/v1")
        );
    }

    #[test]
    fn skips_reserved_key_in_non_official_stored_provider_template() {
        let db = Database::memory().expect("memory db");
        let provider = Provider::with_id(
            "custom-openai".to_string(),
            "Custom OpenAI".to_string(),
            serde_json::json!({
                "auth": {},
                "config": r#"model_provider = "openai"

[model_providers.openai]
name = "Custom OpenAI"
base_url = "https://proxy.example/v1"
"#
            }),
            None,
        );
        db.save_provider("codex", &provider).expect("save provider");

        let (outcome, _backup_dir) = migrate_provider_templates_for_test(&db);
        assert!(outcome.migrated_provider_ids.is_empty());

        let saved = db
            .get_provider_by_id("custom-openai", "codex")
            .expect("get provider")
            .expect("provider exists");
        let config_text = saved
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .expect("config text");
        let parsed: toml::Value = toml::from_str(config_text).expect("parse config");

        assert_eq!(
            parsed
                .get("model_provider")
                .and_then(|value| value.as_str()),
            Some("openai")
        );
        assert_eq!(
            parsed
                .get("model_providers")
                .and_then(|value| value.get("openai"))
                .and_then(|value| value.get("base_url"))
                .and_then(|value| value.as_str()),
            Some("https://proxy.example/v1")
        );
    }

    #[test]
    fn migrates_profile_model_provider_refs_to_custom_when_top_level_is_already_custom() {
        let db = Database::memory().expect("memory db");
        let provider = Provider::with_id(
            "profiled".to_string(),
            "Profiled Relay".to_string(),
            serde_json::json!({
                "auth": {},
                "config": r#"model_provider = "custom"
profile = "work"

[model_providers.custom]
name = "Current"
base_url = "https://current.example/v1"

[model_providers.aihubmix]
name = "AIHubMix"
base_url = "https://aihubmix.example/v1"

[profiles.work]
model_provider = "aihubmix"
"#
            }),
            None,
        );
        db.save_provider("codex", &provider).expect("save provider");

        let (outcome, _backup_dir) = migrate_provider_templates_for_test(&db);
        assert_eq!(outcome.migrated_provider_ids, vec!["profiled".to_string()]);

        let saved = db
            .get_provider_by_id("profiled", "codex")
            .expect("get provider")
            .expect("provider exists");
        let config_text = saved
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .expect("config text");
        let parsed: toml::Value = toml::from_str(config_text).expect("parse config");

        assert_eq!(
            parsed
                .get("profiles")
                .and_then(|value| value.get("work"))
                .and_then(|value| value.get("model_provider"))
                .and_then(|value| value.as_str()),
            Some("custom")
        );
        assert_eq!(
            parsed
                .get("model_providers")
                .and_then(|value| value.get("custom"))
                .and_then(|value| value.get("base_url"))
                .and_then(|value| value.as_str()),
            Some("https://current.example/v1")
        );
    }

    #[test]
    fn skips_custom_category_unknown_provider_when_created_by_cc_switch() {
        let db = Database::memory().expect("memory db");
        let mut provider = Provider::with_id(
            "generated-uuid".to_string(),
            "Manual Relay".to_string(),
            serde_json::json!({
                "auth": {},
                "config": "model_provider = \"my-private-relay\"\n\n[model_providers.my-private-relay]\nname = \"Manual Relay\"\nbase_url = \"http://localhost:8080/v1\""
            }),
            None,
        );
        provider.category = Some("custom".to_string());
        provider.created_at = Some(1);

        db.save_provider("codex", &provider).expect("save provider");

        let ids = collect_source_model_provider_ids(&db).expect("collect ids");
        assert!(!ids.contains("my-private-relay"));
        assert!(!ids.contains("generated-uuid"));
    }

    #[test]
    fn skips_custom_category_unknown_provider_model_provider_id() {
        let db = Database::memory().expect("memory db");
        let mut provider = Provider::with_id(
            "manual".to_string(),
            "Manual Relay".to_string(),
            serde_json::json!({
                "auth": {},
                "config": "model_provider = \"my-local-relay\"\n\n[model_providers.my-local-relay]\nname = \"Manual Relay\"\nbase_url = \"http://localhost:8080/v1\""
            }),
            None,
        );
        provider.category = Some("custom".to_string());

        db.save_provider("codex", &provider).expect("save provider");

        let ids = collect_source_model_provider_ids(&db).expect("collect ids");
        assert!(!ids.contains("my-local-relay"));
    }
}
