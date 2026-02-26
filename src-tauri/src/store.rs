use crate::app_config::MultiAppConfig;
use crate::database::Database;
use crate::error::AppError;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// 全局应用状态
pub struct AppState {
    pub db: Arc<Database>,
    pub config: RwLock<MultiAppConfig>,
}

impl AppState {
    /// 创建新的应用状态
    pub fn try_new() -> Result<Self, AppError> {
        let app_config_dir = crate::config::get_app_config_dir();
        let db_path = app_config_dir.join("cc-switch.db");
        let config_path = app_config_dir.join("config.json");
        let skills_path = app_config_dir.join("skills.json");

        if db_path.exists() {
            let db = Arc::new(Database::init()?);
            let mut config = export_db_to_multi_app_config(&db)?;
            migrate_legacy_codex_configs(&db, &mut config);
            return Ok(Self {
                db,
                config: RwLock::new(config),
            });
        }

        // Validate legacy files before creating the database file.
        let legacy_config = if config_path.exists() {
            Some(MultiAppConfig::load()?)
        } else {
            None
        };

        let legacy_skills_index = if skills_path.exists() {
            Some(load_skills_index_for_migration(&skills_path)?)
        } else {
            None
        };

        // Now create the database and migrate.
        let db = Arc::new(Database::init()?);

        if let Some(config) = legacy_config {
            db.migrate_from_json(&config)?;
            archive_legacy_file(&config_path, "migrated")?;
        }

        if let Some(index) = legacy_skills_index {
            // Migrate legacy skills index flags into upstream-aligned storage:
            // - sync method lives in settings.json
            // - SSOT migration pending lives in DB settings table
            crate::settings::set_skill_sync_method(index.sync_method)?;
            db.set_setting(
                "skills_ssot_migration_pending",
                if index.ssot_migration_pending {
                    "true"
                } else {
                    "false"
                },
            )?;

            // repos
            for repo in &index.repos {
                db.save_skill_repo(repo)?;
            }
            // installed skills
            for skill in index.skills.values() {
                db.save_skill(skill)?;
            }
            archive_legacy_file(&skills_path, "migrated")?;
        }

        // Ensure default repos exist (insert-missing only).
        let _ = db.init_default_skill_repos();

        let mut config = export_db_to_multi_app_config(&db)?;
        migrate_legacy_codex_configs(&db, &mut config);
        Ok(Self {
            db,
            config: RwLock::new(config),
        })
    }

    /// 将内存中的 config 快照持久化到 SQLite（SSOT）。
    pub fn save(&self) -> Result<(), AppError> {
        let config = self.config.read().map_err(AppError::from)?;
        persist_multi_app_config_to_db(&self.db, &config)
    }
}

fn export_db_to_multi_app_config(db: &Database) -> Result<MultiAppConfig, AppError> {
    use crate::app_config::AppType;
    use crate::provider::ProviderManager;

    let mut config = MultiAppConfig::default();

    for app in [AppType::Claude, AppType::Codex, AppType::Gemini] {
        let app_key = app.as_str();
        let providers = db.get_all_providers(app_key)?;
        let current = db.get_current_provider(app_key)?.unwrap_or_default();
        let manager = ProviderManager { providers, current };
        config.apps.insert(app_key.to_string(), manager);

        // prompts
        let prompts = db.get_prompts(app_key)?;
        match app {
            AppType::Claude => config.prompts.claude.prompts = prompts.into_iter().collect(),
            AppType::Codex => config.prompts.codex.prompts = prompts.into_iter().collect(),
            AppType::Gemini => config.prompts.gemini.prompts = prompts.into_iter().collect(),
        }

        // common snippet
        let snippet = db.get_config_snippet(app_key)?;
        config.common_config_snippets.set(&app, snippet);
    }

    // mcp servers (unified)
    let servers = db.get_all_mcp_servers()?;
    config.mcp.servers = Some(servers.into_iter().collect());

    Ok(config)
}

fn persist_multi_app_config_to_db(db: &Database, config: &MultiAppConfig) -> Result<(), AppError> {
    use crate::app_config::AppType;

    for app in [AppType::Claude, AppType::Codex, AppType::Gemini] {
        let app_key = app.as_str();
        let manager = config.get_manager(&app);

        let desired = manager
            .map(|m| {
                m.providers
                    .keys()
                    .cloned()
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();
        let existing = db.get_all_providers(app_key)?;

        // Upsert desired
        if let Some(m) = manager {
            for provider in m.providers.values() {
                db.save_provider(app_key, provider)?;
            }

            if !m.current.trim().is_empty() {
                db.set_current_provider(app_key, &m.current)?;
            }
        }

        // Delete removed (only within supported apps)
        for (id, _) in existing.iter() {
            if !desired.contains(id) {
                db.delete_provider(app_key, id)?;
            }
        }

        // Prompts
        let desired_prompts = match &app {
            AppType::Claude => &config.prompts.claude.prompts,
            AppType::Codex => &config.prompts.codex.prompts,
            AppType::Gemini => &config.prompts.gemini.prompts,
        };
        let existing_prompts = db.get_prompts(app_key)?;
        for prompt in desired_prompts.values() {
            db.save_prompt(app_key, prompt)?;
        }
        for (id, _) in existing_prompts.iter() {
            if !desired_prompts.contains_key(id) {
                db.delete_prompt(app_key, id)?;
            }
        }

        // Common config snippets
        db.set_config_snippet(app_key, config.common_config_snippets.get(&app).cloned())?;
    }

    // MCP servers (global, unified)
    let desired_servers = config.mcp.servers.as_ref().cloned().unwrap_or_default();
    let existing_servers = db.get_all_mcp_servers()?;
    for server in desired_servers.values() {
        db.save_mcp_server(server)?;
    }
    for (id, _) in existing_servers.iter() {
        if !desired_servers.contains_key(id) {
            db.delete_mcp_server(id)?;
        }
    }

    Ok(())
}

fn load_skills_index_for_migration(
    path: &Path,
) -> Result<crate::services::skill::SkillsIndex, AppError> {
    use crate::services::skill::{InstalledSkill, SkillApps, SkillStore, SkillsIndex, SyncMethod};

    let raw = std::fs::read_to_string(path).map_err(|e| AppError::io(path, e))?;
    let raw = raw.trim_start_matches('\u{feff}');
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| AppError::json(path, e))?;

    if value.get("version").and_then(|v| v.as_u64()).is_some() {
        let mut index: SkillsIndex =
            serde_json::from_value(value).map_err(|e| AppError::json(path, e))?;
        if index.version == 0 {
            index.version = 1;
        }
        return Ok(index);
    }

    // Legacy file: SkillStore (Claude-only) -> SkillsIndex
    let legacy: SkillStore = serde_json::from_value(value).map_err(|e| AppError::json(path, e))?;
    let mut index = SkillsIndex {
        version: 1,
        sync_method: SyncMethod::Auto,
        repos: legacy.repos,
        skills: std::collections::HashMap::new(),
        ssot_migration_pending: true,
    };

    for (directory, state) in legacy.skills.into_iter() {
        if !state.installed {
            continue;
        }
        let installed_at = state.installed_at.timestamp();
        let record = InstalledSkill {
            id: format!("local:{directory}"),
            name: directory.clone(),
            description: None,
            directory: directory.clone(),
            readme_url: None,
            repo_owner: None,
            repo_name: None,
            repo_branch: None,
            apps: SkillApps::only(&crate::app_config::AppType::Claude),
            installed_at,
        };
        index.skills.insert(directory, record);
    }

    Ok(index)
}

fn archive_legacy_file(path: &Path, suffix: &str) -> Result<Option<PathBuf>, AppError> {
    if !path.exists() {
        return Ok(None);
    }

    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Config("invalid file name".to_string()))?
        .to_string_lossy()
        .to_string();

    let mut candidate = path.with_file_name(format!("{file_name}.{suffix}"));
    let mut counter: u32 = 1;
    while candidate.exists() {
        candidate = path.with_file_name(format!("{file_name}.{suffix}.{counter}"));
        counter += 1;
    }

    std::fs::rename(path, &candidate).map_err(|e| AppError::io(path, e))?;
    Ok(Some(candidate))
}

/// One-time migration: convert legacy flat Codex configs to the upstream
/// `model_provider + [model_providers.<key>]` format and persist to DB.
///
/// After this runs, all Codex providers in memory and DB use the new format.
fn migrate_legacy_codex_configs(db: &Database, config: &mut MultiAppConfig) {
    use crate::app_config::AppType;
    use crate::services::provider::migrate_legacy_codex_config;

    let manager = match config.get_manager_mut(&AppType::Codex) {
        Some(m) => m,
        None => return,
    };

    for (provider_id, provider) in manager.providers.iter_mut() {
        let cfg_text = match provider
            .settings_config
            .get("config")
            .and_then(|v| v.as_str())
        {
            Some(t) => t,
            None => continue,
        };

        if let Some(migrated) = migrate_legacy_codex_config(cfg_text, provider) {
            // Update in-memory
            if let Some(obj) = provider.settings_config.as_object_mut() {
                obj.insert(
                    "config".to_string(),
                    serde_json::Value::String(migrated),
                );
            }
            // Persist to DB
            if let Err(e) = db.update_provider_settings_config(
                AppType::Codex.as_str(),
                provider_id,
                &provider.settings_config,
            ) {
                log::warn!(
                    "Failed to persist migrated Codex config for provider '{}': {}",
                    provider_id,
                    e
                );
            }
        }
    }
}
