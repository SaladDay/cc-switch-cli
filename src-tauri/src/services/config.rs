use super::provider::ProviderService;
use crate::app_config::{AppType, MultiAppConfig};
use crate::database::Database;
use crate::error::AppError;
use crate::provider::Provider;
use crate::services::{RestoreCompletion, RestoreCoordinator, RestorePostCommitStatus};
use crate::store::AppState;
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_BACKUPS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackupRestoreFormat {
    Sql,
    Sqlite,
}

impl BackupRestoreFormat {
    const fn extension(self) -> &'static str {
        match self {
            Self::Sql => "sql",
            Self::Sqlite => "db",
        }
    }
}

fn validate_backup_id(value: &str) -> Result<(), AppError> {
    crate::skill_directory::validate_portable_component(value).map_err(|error| {
        AppError::InvalidInput(format!("无效的备份 ID {value:?}: {}", error.reason()))
    })
}

fn path_entry_exists(path: &Path) -> Result<bool, AppError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AppError::io(path, error)),
    }
}

/// 备份信息
#[derive(Debug, Clone)]
pub struct BackupInfo {
    /// 备份 ID（文件名不含扩展名）
    pub id: String,
    /// 完整文件路径
    pub path: PathBuf,
    /// 创建时间戳（格式化字符串）
    pub timestamp: String,
    /// 显示名称（用于 UI）
    pub display_name: String,
}

/// 配置导入导出相关业务逻辑
pub struct ConfigService;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfigRestoreResult {
    pub(crate) pre_restore_backup_id: String,
    pub(crate) status: RestorePostCommitStatus,
}

impl ConfigService {
    /// 为当前数据库创建 SQL 备份，返回备份 ID（若数据库不存在则返回空字符串）。
    ///
    /// # 参数
    /// - `config_path`: 兼容参数（忽略），保留给旧调用方
    /// - `custom_name`: 可选的自定义名称
    ///
    /// # 命名规则
    /// - 有自定义名称：`{custom_name}_{timestamp}.sql`
    /// - 无自定义名称：`backup_{timestamp}.sql`
    pub fn create_backup(
        config_path: &Path,
        custom_name: Option<String>,
    ) -> Result<String, AppError> {
        let db_path = crate::config::get_app_config_dir().join("cc-switch.db");
        if !db_path.exists() {
            return Ok(String::new());
        }

        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let backup_id = if let Some(name) = custom_name {
            validate_backup_id(&name)?;
            format!("{}_{}", name, timestamp)
        } else {
            format!("backup_{}", timestamp)
        };
        validate_backup_id(&backup_id)?;

        let backup_dir = config_path
            .parent()
            .or_else(|| db_path.parent())
            .ok_or_else(|| AppError::Config("Invalid config path".into()))?
            .join("backups");

        crate::database::create_secure_dir_all(&backup_dir)?;

        let backup_path = backup_dir.join(format!("{backup_id}.sql"));
        let db = Database::init()?;
        db.export_sql(&backup_path)?;
        crate::config::restrict_file_permissions(&backup_path)
            .map_err(|e| AppError::io(&backup_path, e))?;

        Self::cleanup_old_backups(&backup_dir, MAX_BACKUPS)?;

        Ok(backup_id)
    }

    /// 列出所有可用的备份
    pub fn list_backups(config_path: &Path) -> Result<Vec<BackupInfo>, AppError> {
        let backup_dir = config_path
            .parent()
            .ok_or_else(|| AppError::Config("Invalid config path".into()))?
            .join("backups");

        if !backup_dir.exists() {
            return Ok(Vec::new());
        }

        let entries = fs::read_dir(&backup_dir).map_err(|e| AppError::io(&backup_dir, e))?;

        let mut backups: Vec<BackupInfo> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.path().extension().is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("sql") || extension.eq_ignore_ascii_case("db")
                })
            })
            .filter_map(|entry| {
                let path = entry.path();
                let filename = path.file_stem()?.to_str()?.to_string();

                // 提取时间戳（假设格式为 xxx_YYYYMMDD_HHMMSS）
                let timestamp = Self::extract_timestamp(&filename)?;

                // 生成显示名称
                let display_name = Self::format_display_name(&filename, &timestamp);

                Some(BackupInfo {
                    id: filename.clone(),
                    path: path.clone(),
                    timestamp,
                    display_name,
                })
            })
            .collect();

        // 按时间戳降序排序（最新的在前）
        backups.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        Ok(backups)
    }

    /// 根据备份 ID 恢复配置
    pub fn restore_from_backup_id(backup_id: &str) -> Result<String, AppError> {
        Self::restore_from_backup_id_with_status(backup_id)
            .map(|result| result.pre_restore_backup_id)
    }

    pub(crate) fn restore_from_backup_id_with_status(
        backup_id: &str,
    ) -> Result<ConfigRestoreResult, AppError> {
        Self::restore_from_backup_id_and_then(backup_id, |_| ()).map(ConfigRestoreResult::from)
    }

    pub(crate) fn restore_from_backup_id_and_then<T>(
        backup_id: &str,
        after_restore: impl FnOnce(&AppState) -> T,
    ) -> Result<RestoreCompletion<T>, AppError> {
        validate_backup_id(backup_id)?;
        let config_path = crate::config::get_app_config_path();
        let backup_dir = config_path
            .parent()
            .ok_or_else(|| AppError::Config("Invalid config path".into()))?
            .join("backups");
        crate::config::create_managed_config_dir_all(&backup_dir)?;

        let sql_path = backup_dir.join(format!(
            "{backup_id}.{}",
            BackupRestoreFormat::Sql.extension()
        ));
        let sqlite_path = backup_dir.join(format!(
            "{backup_id}.{}",
            BackupRestoreFormat::Sqlite.extension()
        ));
        let sql_exists = path_entry_exists(&sql_path)?;
        let sqlite_exists = path_entry_exists(&sqlite_path)?;
        let (backup_path, format) = match (sql_exists, sqlite_exists) {
            (true, false) => (sql_path, BackupRestoreFormat::Sql),
            (false, true) => (sqlite_path, BackupRestoreFormat::Sqlite),
            (false, false) => {
                return Err(AppError::Message(format!("备份文件不存在: {backup_id}")));
            }
            (true, true) => {
                return Err(AppError::InvalidInput(format!(
                    "备份 ID 同时匹配 SQL 和 SQLite 文件，拒绝歧义恢复: {backup_id}"
                )));
            }
        };

        Self::restore_database_path_and_then(&backup_path, format, after_restore)
    }

    /// 从文件名提取时间戳字符串
    fn extract_timestamp(filename: &str) -> Option<String> {
        // Manual SQL backups end at the timestamp; SQLite safety snapshots
        // append subsecond/process uniqueness fields. Find the date/time pair
        // instead of assuming it is the final pair.
        filename
            .split('_')
            .collect::<Vec<_>>()
            .windows(2)
            .rev()
            .find(|parts| {
                parts[0].len() == 8
                    && parts[1].len() == 6
                    && parts[0].bytes().all(|byte| byte.is_ascii_digit())
                    && parts[1].bytes().all(|byte| byte.is_ascii_digit())
            })
            .map(|parts| format!("{}_{}", parts[0], parts[1]))
    }

    /// 格式化显示名称
    fn format_display_name(filename: &str, timestamp: &str) -> String {
        // 从时间戳格式 YYYYMMDD_HHMMSS 转换为可读格式
        if timestamp.len() == 15 {
            // YYYYMMDD_HHMMSS
            let date = &timestamp[0..8];
            let time = &timestamp[9..15];

            if let (Ok(y), Ok(m), Ok(d), Ok(h), Ok(min), Ok(s)) = (
                date[0..4].parse::<u32>(),
                date[4..6].parse::<u32>(),
                date[6..8].parse::<u32>(),
                time[0..2].parse::<u32>(),
                time[2..4].parse::<u32>(),
                time[4..6].parse::<u32>(),
            ) {
                let formatted_time =
                    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, h, min, s);

                // 如果是自定义名称，显示名称和时间
                if !filename.starts_with("backup_") {
                    let custom_name = filename.rsplitn(3, '_').nth(2).unwrap_or(filename);
                    return format!("{} ({})", custom_name, formatted_time);
                }

                return formatted_time;
            }
        }

        // 回退：直接返回文件名
        filename.to_string()
    }

    fn cleanup_old_backups(backup_dir: &Path, retain: usize) -> Result<(), AppError> {
        if retain == 0 {
            return Ok(());
        }

        let entries = match fs::read_dir(backup_dir) {
            Ok(iter) => iter
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .map(|ext| ext == "sql")
                        .unwrap_or(false)
                })
                .collect::<Vec<_>>(),
            Err(_) => return Ok(()),
        };

        if entries.len() <= retain {
            return Ok(());
        }

        let remove_count = entries.len().saturating_sub(retain);
        let mut sorted = entries;

        sorted.sort_by(|a, b| {
            let a_time = a.metadata().and_then(|m| m.modified()).ok();
            let b_time = b.metadata().and_then(|m| m.modified()).ok();
            a_time.cmp(&b_time)
        });

        for entry in sorted.into_iter().take(remove_count) {
            if let Err(err) = fs::remove_file(entry.path()) {
                log::warn!(
                    "Failed to remove old backup {}: {}",
                    entry.path().display(),
                    err
                );
            }
        }

        Ok(())
    }

    /// 将当前 config.json 拷贝到目标路径。
    pub fn export_config_to_path(target_path: &Path) -> Result<(), AppError> {
        let db = Database::init()?;
        db.export_sql(target_path)
    }

    pub fn import_config_from_path(file_path: &Path) -> Result<String, AppError> {
        Self::import_config_from_path_with_status(file_path)
            .map(|result| result.pre_restore_backup_id)
    }

    pub(crate) fn import_config_from_path_with_status(
        file_path: &Path,
    ) -> Result<ConfigRestoreResult, AppError> {
        Self::import_config_from_path_and_then(file_path, |_| ()).map(ConfigRestoreResult::from)
    }

    pub(crate) fn import_config_from_path_and_then<T>(
        file_path: &Path,
        after_import: impl FnOnce(&AppState) -> T,
    ) -> Result<RestoreCompletion<T>, AppError> {
        Self::restore_database_path_and_then(file_path, BackupRestoreFormat::Sql, after_import)
    }

    fn restore_database_path_and_then<T>(
        file_path: &Path,
        format: BackupRestoreFormat,
        after_import: impl FnOnce(&AppState) -> T,
    ) -> Result<RestoreCompletion<T>, AppError> {
        let restore = RestoreCoordinator::acquire_blocking()?;
        let state = restore.load_state()?;
        let db_path = crate::config::get_app_config_dir().join("cc-switch.db");
        if !db_path.exists() {
            return Err(AppError::Config("数据库不存在，无法导入".to_string()));
        }

        let prepared = match format {
            BackupRestoreFormat::Sql => Database::prepare_sql_restore(file_path)?,
            BackupRestoreFormat::Sqlite => Database::prepare_binary_restore(file_path)?,
        };
        restore.publish(&state, prepared, None, after_import)
    }

    /// 同步当前供应商到对应的 live 配置。
    pub fn sync_current_providers_to_live(config: &mut MultiAppConfig) -> Result<(), AppError> {
        Self::sync_current_provider_for_app(config, &AppType::Claude)?;
        Self::sync_current_provider_for_app(config, &AppType::Codex)?;
        Self::sync_current_provider_for_app(config, &AppType::Gemini)?;
        Self::sync_current_provider_for_app(config, &AppType::OpenCode)?;
        Self::sync_current_provider_for_app(config, &AppType::Hermes)?;
        Self::sync_current_provider_for_app(config, &AppType::OpenClaw)?;
        Ok(())
    }

    fn sync_current_provider_for_app(
        config: &mut MultiAppConfig,
        app_type: &AppType,
    ) -> Result<(), AppError> {
        let (current_id, provider) = {
            let manager = match config.get_manager(app_type) {
                Some(manager) => manager,
                None => return Ok(()),
            };

            if manager.current.is_empty() {
                return Ok(());
            }

            let current_id = manager.current.clone();
            let provider = match manager.providers.get(&current_id) {
                Some(provider) => provider.clone(),
                None => {
                    log::warn!(
                        "当前应用 {app_type:?} 的供应商 {current_id} 不存在，跳过 live 同步"
                    );
                    return Ok(());
                }
            };
            (current_id, provider)
        };

        match app_type {
            AppType::Codex => Self::sync_codex_live(config, &current_id, &provider)?,
            AppType::Claude => Self::sync_claude_live(config, &current_id, &provider)?,
            AppType::Gemini => Self::sync_gemini_live(config, &current_id, &provider)?,
            AppType::OpenCode => {}
            AppType::Hermes => {}
            AppType::OpenClaw => {}
        }

        Ok(())
    }

    fn sync_codex_live(
        config: &mut MultiAppConfig,
        provider_id: &str,
        provider: &Provider,
    ) -> Result<(), AppError> {
        let common_config_snippet = config.common_config_snippets.codex.clone();
        let apply_common_config = ProviderService::provider_uses_common_config_for_app(
            &AppType::Codex,
            provider,
            common_config_snippet.as_deref(),
        );
        ProviderService::write_codex_live_force(
            provider,
            common_config_snippet.as_deref(),
            apply_common_config,
        )?;
        crate::mcp::sync_enabled_to_codex(config)?;

        let auth_path = crate::codex_config::get_codex_auth_path();
        let auth_after = if auth_path.exists() {
            crate::config::read_json_file(&auth_path)?
        } else {
            serde_json::json!({})
        };
        let cfg_text_after = crate::codex_config::read_and_validate_codex_config_text()?;
        if let Some(manager) = config.get_manager_mut(&AppType::Codex) {
            if let Some(target) = manager.providers.get_mut(provider_id) {
                let mut restored = serde_json::json!({
                    "auth": auth_after,
                    "config": cfg_text_after,
                });
                let restore_provider_token =
                    crate::codex_config::should_restore_codex_provider_token_for_backfill(
                        ProviderService::codex_live_write_category(provider),
                        &provider.settings_config,
                    );
                crate::codex_config::restore_codex_settings_for_backfill(
                    &mut restored,
                    &provider.settings_config,
                    restore_provider_token,
                )?;
                if ProviderService::codex_live_write_category(provider) == Some("official") {
                    crate::codex_config::strip_codex_unified_session_bucket_from_settings(
                        &mut restored,
                    )?;
                }
                // Guarantee a non-official snapshot never stores ChatGPT OAuth
                // material — even if the token-restore step above rebuilt auth
                // from an already-polluted stored template (issue #328).
                if ProviderService::codex_live_write_category(provider) != Some("official") {
                    if let Some(obj) = restored.as_object_mut() {
                        let sanitized = crate::codex_config::sanitize_codex_third_party_auth(
                            obj.get("auth"),
                            obj.get("config").and_then(serde_json::Value::as_str),
                            None,
                            None,
                        );
                        obj.insert("auth".to_string(), sanitized);
                    }
                }
                target.settings_config = ProviderService::normalize_settings_config_for_storage(
                    &AppType::Codex,
                    provider,
                    restored,
                    common_config_snippet.as_deref(),
                )?;
            }
        }

        Ok(())
    }

    fn sync_claude_live(
        config: &mut MultiAppConfig,
        provider_id: &str,
        provider: &Provider,
    ) -> Result<(), AppError> {
        let common_config_snippet = config.common_config_snippets.claude.clone();
        let apply_common_config = ProviderService::provider_uses_common_config_for_app(
            &AppType::Claude,
            provider,
            common_config_snippet.as_deref(),
        );
        ProviderService::write_claude_live_force(
            provider,
            common_config_snippet.as_deref(),
            apply_common_config,
        )?;

        let settings_path = crate::config::get_claude_settings_path();
        let live_after = crate::config::read_json_file::<serde_json::Value>(&settings_path)?;
        if let Some(manager) = config.get_manager_mut(&AppType::Claude) {
            if let Some(target) = manager.providers.get_mut(provider_id) {
                target.settings_config = ProviderService::normalize_settings_config_for_storage(
                    &AppType::Claude,
                    provider,
                    live_after,
                    common_config_snippet.as_deref(),
                )?;
            }
        }

        Ok(())
    }

    fn sync_gemini_live(
        config: &mut MultiAppConfig,
        provider_id: &str,
        provider: &Provider,
    ) -> Result<(), AppError> {
        use crate::gemini_config::{env_to_json, read_gemini_env};

        let common_config_snippet = config.common_config_snippets.gemini.clone();
        let common_config_snippet_to_apply = if ProviderService::provider_uses_common_config_for_app(
            &AppType::Gemini,
            provider,
            common_config_snippet.as_deref(),
        ) {
            common_config_snippet.as_deref()
        } else {
            None
        };
        ProviderService::write_gemini_live_force(provider, common_config_snippet_to_apply)?;

        // 读回实际写入的内容并更新到配置中（包含 settings.json）
        let live_after_env = read_gemini_env()?;
        let settings_path = crate::gemini_config::get_gemini_settings_path();
        let live_after_config = if settings_path.exists() {
            crate::config::read_json_file(&settings_path)?
        } else {
            serde_json::json!({})
        };
        let mut live_after = env_to_json(&live_after_env);
        if let Some(obj) = live_after.as_object_mut() {
            obj.insert("config".to_string(), live_after_config);
        }

        if let Some(manager) = config.get_manager_mut(&AppType::Gemini) {
            if let Some(target) = manager.providers.get_mut(provider_id) {
                target.settings_config = ProviderService::normalize_settings_config_for_storage(
                    &AppType::Gemini,
                    provider,
                    live_after,
                    common_config_snippet.as_deref(),
                )?;
            }
        }

        Ok(())
    }
}

impl ConfigRestoreResult {
    pub(crate) fn pending_retry_message(&self) -> Option<String> {
        self.status.pending_retry().map(|pending| pending.message())
    }
}

impl From<RestoreCompletion<()>> for ConfigRestoreResult {
    fn from(completion: RestoreCompletion<()>) -> Self {
        Self {
            pre_restore_backup_id: completion.pre_restore_backup_id,
            status: completion.status,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_backup_id, ConfigService};
    use crate::app_config::AppType;
    use crate::provider::Provider;
    use crate::services::state_coordination::acquire_restore_exclusive_permit;
    use crate::store::AppState;
    use crate::{AppError, Database};
    use serde_json::json;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn backup_ids_are_portable_single_path_components() {
        for invalid in [
            "",
            ".",
            "..",
            "../outside",
            r"..\outside",
            "/absolute",
            r"C:\absolute",
            "CON",
            "trailing.",
            "trailing ",
        ] {
            assert!(
                validate_backup_id(invalid).is_err(),
                "{invalid:?} must not become a backup path"
            );
        }
        validate_backup_id("db_backup_20260802_073831_1_2_3")
            .expect("generated safety backup id is portable");
    }

    #[test]
    fn local_config_import_waits_for_the_shared_restore_mutation_guard() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create isolated config import home");
        let _environment = crate::test_support::TestEnvGuard::isolated(temp.path());

        let state = AppState::try_new()?;
        let import_db = Database::memory()?;
        import_db.save_provider(
            AppType::Claude.as_str(),
            &Provider::with_id(
                "imported-provider".to_string(),
                "Imported Provider".to_string(),
                json!({"env": {"ANTHROPIC_AUTH_TOKEN": "sandbox-token"}}),
                None,
            ),
        )?;
        let import_path = temp.path().join("import.sql");
        import_db.export_sql(&import_path)?;
        drop(state);

        let restore_guard = futures::executor::block_on(acquire_restore_exclusive_permit())
            .map_err(AppError::Message)?;
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();

        std::thread::scope(|scope| {
            scope.spawn(|| {
                started_tx.send(()).expect("signal import start");
                let result = ConfigService::import_config_from_path(&import_path);
                finished_tx.send(result).expect("report import result");
            });

            started_rx.recv().expect("import worker should start");
            assert!(
                matches!(
                    finished_rx.recv_timeout(Duration::from_secs(2)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ),
                "local imports must serialize behind the same guard as cloud restores"
            );

            drop(restore_guard);
            finished_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("import should resume after releasing the restore guard")
                .expect("guarded import should succeed");
        });

        Ok(())
    }

    #[test]
    fn local_config_import_reuses_cloud_restore_proxy_preflight() -> Result<(), AppError> {
        let temp = tempfile::tempdir().expect("create isolated config import home");
        let _environment = crate::test_support::TestEnvGuard::isolated(temp.path());

        let state = AppState::try_new()?;
        let mut proxy_config = futures::executor::block_on(
            state.db.get_proxy_config_for_app(AppType::Claude.as_str()),
        )?;
        proxy_config.enabled = true;
        futures::executor::block_on(state.db.update_proxy_config_for_app(proxy_config))?;

        let import_db = Database::memory()?;
        import_db.save_provider(
            AppType::Claude.as_str(),
            &Provider::with_id(
                "must-not-publish".to_string(),
                "Blocked Restore".to_string(),
                json!({"env": {"ANTHROPIC_AUTH_TOKEN": "sandbox-token"}}),
                None,
            ),
        )?;
        let import_path = temp.path().join("blocked-import.sql");
        import_db.export_sql(&import_path)?;
        drop(state);

        let error = ConfigService::import_config_from_path(&import_path)
            .expect_err("local restore must reject an active proxy takeover");
        assert!(
            error.to_string().contains("proxy") || error.to_string().contains("代理"),
            "the common restore preflight should explain the active proxy boundary: {error}"
        );
        let state = AppState::try_new()?;
        assert!(
            state
                .db
                .get_provider_by_id("must-not-publish", AppType::Claude.as_str())?
                .is_none(),
            "preflight rejection must happen before canonical publication"
        );
        Ok(())
    }
}
