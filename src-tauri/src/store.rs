use crate::app_config::MultiAppConfig;
use crate::database::Database;
use crate::error::AppError;
use crate::services::ProxyService;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// 全局应用状态
pub struct AppState {
    pub db: Arc<Database>,
    pub config: RwLock<MultiAppConfig>,
    pub proxy_service: ProxyService,
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
            crate::services::provider::ProviderService::migrate_common_config_upstream_semantics_if_needed(
                &db,
                &mut config,
            )?;
            return Self::from_parts(db, config);
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
        crate::services::provider::ProviderService::migrate_common_config_upstream_semantics_if_needed(
            &db,
            &mut config,
        )?;
        Self::from_parts(db, config)
    }

    /// 打开只读数据库快照，用于 TUI 后台热刷新等非初始化路径。
    pub fn try_open_snapshot() -> Result<Self, AppError> {
        let db = Arc::new(Database::open_readonly_current_schema()?);
        let config = export_db_to_multi_app_config(&db)?;
        Self::from_parts(db, config)
    }

    /// 创建新的应用状态，并在真实进程启动路径上执行一次启动恢复。
    pub fn try_new_with_startup_recovery() -> Result<Self, AppError> {
        let state = Self::try_new()?;

        state.import_live_provider_configs_on_startup()?;

        state.initialize_common_config_snippets();

        let owned_managed_session_active = state
            .proxy_service
            .should_skip_startup_recovery_for_active_managed_session_blocking()
            .map_err(AppError::Message)?;
        if !owned_managed_session_active {
            let proxy_running = state
                .proxy_service
                .is_running_blocking()
                .map_err(AppError::Message)?;
            if !proxy_running {
                state
                    .proxy_service
                    .recover_takeovers_on_startup_blocking()
                    .map_err(AppError::Config)?;
            }
        }

        state.import_live_current_provider_configs_on_startup()?;
        state.migrate_codex_provider_buckets_on_startup();

        Ok(state)
    }

    fn migrate_codex_provider_buckets_on_startup(&self) {
        match crate::codex_history_migration::maybe_migrate_codex_third_party_history_provider_bucket(
            &self.db,
        ) {
            Ok(outcome) => {
                if let Some(reason) = outcome.skipped_reason {
                    log::debug!("○ Codex history provider bucket migration skipped: {reason}");
                } else {
                    log::info!(
                        "✓ Codex history provider bucket migration completed: sources={}, jsonl_files={}, state_rows={}",
                        outcome.source_provider_ids.len(),
                        outcome.migrated_jsonl_files,
                        outcome.migrated_state_rows
                    );
                }
            }
            Err(error) => {
                log::warn!("✗ Codex history provider bucket migration failed: {error}");
            }
        }

        match crate::codex_history_migration::maybe_migrate_codex_provider_template_bucket(&self.db)
        {
            Ok(outcome) => {
                if let Some(reason) = outcome.skipped_reason {
                    log::debug!("○ Codex provider template bucket migration skipped: {reason}");
                } else if !outcome.migrated_provider_ids.is_empty() {
                    log::info!(
                        "✓ Codex provider template bucket migration completed: providers={}",
                        outcome.migrated_provider_ids.len()
                    );
                }
            }
            Err(error) => {
                log::warn!("✗ Codex provider template bucket migration failed: {error}");
            }
        }

        match crate::codex_history_migration::maybe_migrate_codex_official_history_to_unified_bucket(
        ) {
            Ok(outcome) => {
                if let Some(reason) = outcome.skipped_reason {
                    log::debug!("○ Codex official history unify migration skipped: {reason}");
                } else {
                    log::info!(
                        "✓ Codex official history unify migration completed: jsonl_files={}, state_rows={}",
                        outcome.migrated_jsonl_files,
                        outcome.migrated_state_rows
                    );
                }
            }
            Err(error) => {
                log::warn!("✗ Codex official history unify migration failed: {error}");
            }
        }

        if let Err(error) = self.refresh_config_from_db() {
            log::warn!("✗ Failed to refresh config after Codex provider bucket migration: {error}");
        }
    }

    fn import_live_provider_configs_on_startup(&self) -> Result<(), AppError> {
        match self.db.init_default_official_providers() {
            Ok(count) if count > 0 => log::info!("✓ Seeded {count} official provider(s)"),
            Ok(_) => {}
            Err(error) => log::warn!("✗ Failed to seed official providers: {error}"),
        }

        match crate::services::provider::ProviderService::import_opencode_providers_from_live(self)
        {
            Ok(count) if count > 0 => {
                log::info!("✓ Imported {count} OpenCode provider(s) from live config");
            }
            Ok(_) => log::debug!("○ No new OpenCode providers to import"),
            Err(error) => log::warn!("✗ Failed to import OpenCode providers: {error}"),
        }

        match crate::services::provider::ProviderService::import_hermes_providers_from_live(self) {
            Ok(count) if count > 0 => {
                log::info!("✓ Imported {count} Hermes provider(s) from live config");
            }
            Ok(_) => log::debug!("○ No new Hermes providers to import"),
            Err(error) => log::warn!("✗ Failed to import Hermes providers: {error}"),
        }

        match crate::services::provider::ProviderService::import_openclaw_providers_from_live(self)
        {
            Ok(count) if count > 0 => {
                log::info!("✓ Imported {count} OpenClaw provider(s) from live config");
            }
            Ok(_) => log::debug!("○ No new OpenClaw providers to import"),
            Err(error) => log::warn!("✗ Failed to import OpenClaw providers: {error}"),
        }

        self.refresh_config_from_db()
    }

    /// 首次配置时从干净的 live 文件自动播种通用配置片段（Claude + Codex + Gemini）。
    ///
    /// 仅当某 app 的通用配置为空且未被用户显式清空时执行；读取 live 文件失败或
    /// 无可提取内容时静默跳过。必须在代理接管恢复之前调用，避免读到占位符配置。
    ///
    /// OpenCode/Hermes/OpenClaw 不在此列：这三个 app 的通用配置合并机制
    /// （`apply_common_config_to_settings` / `remove_common_config_from_settings`）
    /// 是 no-op，自动播种对它们没有实际效果，反而可能把整份 live 配置（含密钥）
    /// 复制进 `common_config_<app>`。
    fn initialize_common_config_snippets(&self) {
        use crate::app_config::AppType;
        use crate::services::provider::ProviderService;

        let mut seeded = false;

        for app_type in [AppType::Claude, AppType::Codex, AppType::Gemini] {
            match self
                .db
                .should_auto_extract_config_snippet(app_type.as_str())
            {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    log::warn!(
                        "✗ Failed to check auto-extract gate for {}: {error}",
                        app_type.as_str()
                    );
                    continue;
                }
            }

            let settings = match ProviderService::read_live_settings(app_type.clone()) {
                Ok(settings) => settings,
                Err(_) => continue,
            };

            let snippet = match ProviderService::extract_common_config_snippet_from_settings(
                app_type.clone(),
                &settings,
            ) {
                Ok(snippet) => snippet,
                Err(error) => {
                    log::warn!(
                        "✗ Failed to extract common config snippet for {}: {error}",
                        app_type.as_str()
                    );
                    continue;
                }
            };

            if snippet.is_empty() || snippet == "{}" {
                log::debug!(
                    "○ Live config for {} has no extractable common fields",
                    app_type.as_str()
                );
                continue;
            }

            match self.db.set_config_snippet(app_type.as_str(), Some(snippet)) {
                Ok(()) => {
                    let _ = self.db.set_config_snippet_cleared(app_type.as_str(), false);
                    seeded = true;
                    log::info!(
                        "✓ Auto-extracted common config snippet for {}",
                        app_type.as_str()
                    );
                }
                Err(error) => log::warn!(
                    "✗ Failed to save common config snippet for {}: {error}",
                    app_type.as_str()
                ),
            }
        }

        if seeded {
            if let Err(error) = self.refresh_config_from_db() {
                log::warn!(
                    "✗ Failed to refresh config after seeding common config snippets: {error}"
                );
            }
        }
    }

    fn import_live_current_provider_configs_on_startup(&self) -> Result<(), AppError> {
        for app_type in crate::app_config::AppType::all().filter(|app| !app.is_additive_mode()) {
            if self
                .proxy_service
                .detect_takeover_in_live_config_for_app(&app_type)
            {
                log::debug!(
                    "○ {} live config is in proxy takeover mode; live import skipped",
                    app_type.as_str()
                );
                continue;
            }

            match crate::services::provider::ProviderService::import_default_config(
                self,
                app_type.clone(),
            ) {
                Ok(true) => log::info!(
                    "✓ Imported live config for {} as default provider",
                    app_type.as_str()
                ),
                Ok(false) => log::debug!(
                    "○ {} already has providers; live import skipped",
                    app_type.as_str()
                ),
                Err(error) => log::debug!(
                    "○ No live config to import for {}: {error}",
                    app_type.as_str()
                ),
            }
        }

        self.refresh_config_from_db()
    }

    /// 将内存中的 config 快照持久化到 SQLite（SSOT）。
    pub fn save(&self) -> Result<(), AppError> {
        let config = self.config.read().map_err(AppError::from)?;
        persist_multi_app_config_to_db(&self.db, &config)
    }

    pub(crate) fn save_config_snapshot(&self, config: &MultiAppConfig) -> Result<(), AppError> {
        persist_multi_app_config_to_db(&self.db, config)
    }

    /// 将内存中的 config 快照持久化到 SQLite，但保留指定应用当前供应商的 DB 选择。
    pub fn save_preserving_current_providers(
        &self,
        app_types: &[crate::app_config::AppType],
    ) -> Result<(), AppError> {
        let config = self.config.read().map_err(AppError::from)?;
        persist_multi_app_config_to_db_preserving_current_providers(&self.db, &config, app_types)
    }

    pub(crate) fn save_config_snapshot_preserving_current_providers(
        &self,
        config: &MultiAppConfig,
        app_types: &[crate::app_config::AppType],
    ) -> Result<(), AppError> {
        persist_multi_app_config_to_db_preserving_current_providers(&self.db, config, app_types)
    }

    /// 用数据库中的最新快照重建内存配置，供导入/恢复后的 live 同步流程复用。
    pub fn refresh_config_from_db(&self) -> Result<(), AppError> {
        let mut config = export_db_to_multi_app_config(&self.db)?;
        migrate_legacy_codex_configs(&self.db, &mut config);
        crate::services::provider::ProviderService::migrate_common_config_upstream_semantics_if_needed(
            &self.db,
            &mut config,
        )?;

        let mut guard = self.config.write().map_err(AppError::from)?;
        *guard = config;
        Ok(())
    }

    /// 从数据库重建内存配置快照，但不执行任何 legacy/common-config 写入迁移。
    pub fn reload_config_snapshot_from_db(&self) -> Result<(), AppError> {
        let config = export_db_to_multi_app_config(&self.db)?;
        let mut guard = self.config.write().map_err(AppError::from)?;
        *guard = config;
        Ok(())
    }

    fn from_parts(db: Arc<Database>, config: MultiAppConfig) -> Result<Self, AppError> {
        let proxy_service = ProxyService::new(db.clone());

        Ok(Self {
            db,
            config: RwLock::new(config),
            proxy_service,
        })
    }
}

fn export_db_to_multi_app_config(db: &Database) -> Result<MultiAppConfig, AppError> {
    use crate::app_config::AppType;
    use crate::provider::ProviderManager;

    let mut config = MultiAppConfig::default();

    for app in [
        AppType::Claude,
        AppType::Codex,
        AppType::Gemini,
        AppType::OpenCode,
        AppType::Hermes,
        AppType::OpenClaw,
    ] {
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
            AppType::OpenCode => config.prompts.opencode.prompts = prompts.into_iter().collect(),
            AppType::Hermes => config.prompts.hermes.prompts = prompts.into_iter().collect(),
            AppType::OpenClaw => config.prompts.openclaw.prompts = prompts.into_iter().collect(),
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
    persist_multi_app_config_to_db_preserving_current_providers(db, config, &[])
}

fn persist_multi_app_config_to_db_preserving_current_providers(
    db: &Database,
    config: &MultiAppConfig,
    app_types: &[crate::app_config::AppType],
) -> Result<(), AppError> {
    use crate::app_config::AppType;

    let preserved_current_apps = app_types
        .iter()
        .map(crate::app_config::AppType::as_str)
        .collect::<std::collections::HashSet<_>>();

    for app in [
        AppType::Claude,
        AppType::Codex,
        AppType::Gemini,
        AppType::OpenCode,
        AppType::Hermes,
        AppType::OpenClaw,
    ] {
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

            if !preserved_current_apps.contains(app_key) && !m.current.trim().is_empty() {
                db.set_current_provider(app_key, &m.current)?;
            }
        }

        // Delete removed (only within supported apps)
        for (id, _) in existing.iter() {
            if !desired.contains(id) {
                db.delete_provider(app_key, id)?;
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
                obj.insert("config".to_string(), serde_json::Value::String(migrated));
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

#[cfg(test)]
mod tests {
    use super::AppState;
    use crate::test_support::TestEnvGuard;
    use serde_json::json;
    use serial_test::serial;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_text(path: PathBuf, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(path, text).expect("write text file");
    }

    fn write_json(path: PathBuf, value: serde_json::Value) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(
            path,
            serde_json::to_string_pretty(&value).expect("serialize json"),
        )
        .expect("write json file");
    }

    #[test]
    #[serial(home_settings)]
    fn startup_imports_existing_claude_live_config_as_default_provider() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        write_json(
            crate::config::get_claude_settings_path(),
            json!({
                "env": { "ANTHROPIC_API_KEY": "live-key" },
                "permissions": { "allow": ["Bash"] }
            }),
        );

        let state = AppState::try_new_with_startup_recovery().expect("create startup state");
        let provider = state
            .db
            .get_provider_by_id("default", "claude")
            .expect("read provider")
            .expect("default provider should be imported");

        assert_eq!(
            state
                .db
                .get_current_provider("claude")
                .expect("read current provider")
                .as_deref(),
            Some("default")
        );
        assert_eq!(
            provider.settings_config["env"]["ANTHROPIC_API_KEY"],
            json!("live-key")
        );
        assert!(provider.settings_config["permissions"]["allow"].is_array());
        assert!(state
            .db
            .get_provider_by_id("claude-official", "claude")
            .expect("read official provider")
            .is_some());
        let config = state.config.read().expect("read refreshed config");
        let manager = config
            .get_manager(&crate::app_config::AppType::Claude)
            .expect("claude manager");
        assert_eq!(manager.current, "default");
        assert!(manager.providers.contains_key("default"));
        assert!(manager.providers.contains_key("claude-official"));
    }

    #[test]
    #[serial(home_settings)]
    fn startup_imports_existing_codex_live_config_as_default_provider() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        write_json(
            crate::codex_config::get_codex_auth_path(),
            json!({ "OPENAI_API_KEY": "live-codex-key" }),
        );
        write_text(
            crate::codex_config::get_codex_config_path(),
            r#"model_provider = "legacy"
model = "gpt-4"

[model_providers.legacy]
base_url = "https://api.example.com/v1"
wire_api = "responses"
"#,
        );

        let state = AppState::try_new_with_startup_recovery().expect("create startup state");
        let provider = state
            .db
            .get_provider_by_id("default", "codex")
            .expect("read provider")
            .expect("default provider should be imported");

        assert_eq!(
            state
                .db
                .get_current_provider("codex")
                .expect("read current provider")
                .as_deref(),
            Some("default")
        );
        assert_eq!(
            provider.settings_config["auth"]["OPENAI_API_KEY"],
            json!("live-codex-key")
        );
        assert!(provider
            .settings_config
            .get("config")
            .and_then(|value| value.as_str())
            .is_some_and(|text| text.contains("model_provider = \"legacy\"")));
        assert!(state
            .db
            .get_provider_by_id("codex-official", "codex")
            .expect("read official provider")
            .is_some());
        let config = state.config.read().expect("read refreshed config");
        let manager = config
            .get_manager(&crate::app_config::AppType::Codex)
            .expect("codex manager");
        assert_eq!(manager.current, "default");
        assert!(manager.providers.contains_key("default"));
        assert!(manager.providers.contains_key("codex-official"));
    }

    #[test]
    #[serial(home_settings)]
    fn startup_migrates_imported_codex_live_provider_after_import() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        write_json(
            crate::codex_config::get_codex_auth_path(),
            json!({ "OPENAI_API_KEY": "live-codex-key" }),
        );
        write_text(
            crate::codex_config::get_codex_config_path(),
            r#"model_provider = "rightcode"
model = "gpt-5.4"

[model_providers.rightcode]
name = "RightCode"
base_url = "https://rightcode.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#,
        );

        let state = AppState::try_new_with_startup_recovery().expect("create startup state");
        let provider = state
            .db
            .get_provider_by_id("default", "codex")
            .expect("read provider")
            .expect("default provider should be imported before migration");
        let config_text = provider
            .settings_config
            .get("config")
            .and_then(|value| value.as_str())
            .expect("codex provider config");
        assert!(
            config_text.contains("model_provider = \"custom\""),
            "imported live provider should be migrated to the unified custom bucket"
        );
        assert!(
            config_text.contains("[model_providers.custom]"),
            "provider table should be migrated to custom"
        );
        assert!(
            !config_text.contains("[model_providers.rightcode]"),
            "legacy provider table should not remain after migration"
        );

        let migration = crate::settings::get_settings()
            .local_migrations
            .and_then(|migrations| migrations.codex_third_party_history_provider_bucket_v1)
            .expect("history migration should be marked after imported provider is visible");
        assert_eq!(migration.target_provider_id, "custom");
        assert!(
            migration
                .source_provider_ids
                .iter()
                .any(|provider_id| provider_id == "rightcode"),
            "startup migration should collect source ids from the imported live provider"
        );
    }

    #[test]
    #[serial(home_settings)]
    fn startup_seeds_official_providers_when_live_config_is_absent() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        let state = AppState::try_new_with_startup_recovery().expect("create startup state");

        for (app, provider_id, name) in [
            ("claude", "claude-official", "Claude Official"),
            ("codex", "codex-official", "OpenAI Official"),
            ("gemini", "gemini-official", "Google Official"),
        ] {
            let provider = state
                .db
                .get_provider_by_id(provider_id, app)
                .expect("read official provider")
                .unwrap_or_else(|| panic!("{provider_id} should be seeded"));
            assert_eq!(provider.name, name);
            assert_eq!(provider.category.as_deref(), Some("official"));
        }
    }

    #[test]
    #[serial(home_settings)]
    fn import_default_config_runs_when_only_official_seed_exists() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        let state = AppState::try_new_with_startup_recovery().expect("create startup state");
        assert!(state
            .db
            .get_provider_by_id("claude-official", "claude")
            .expect("read official provider")
            .is_some());

        write_json(
            crate::config::get_claude_settings_path(),
            json!({
                "env": { "ANTHROPIC_API_KEY": "late-live-key" }
            }),
        );

        let imported = crate::services::ProviderService::import_default_config(
            &state,
            crate::app_config::AppType::Claude,
        )
        .expect("import live config");

        assert!(imported);
        assert_eq!(
            state
                .db
                .get_current_provider("claude")
                .expect("read current provider")
                .as_deref(),
            Some("default")
        );
    }

    #[test]
    #[serial(home_settings)]
    fn startup_seeds_claude_common_config_snippet_from_live() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        write_json(
            crate::config::get_claude_settings_path(),
            json!({
                "env": { "ANTHROPIC_API_KEY": "live-key", "ANTHROPIC_BASE_URL": "https://x" },
                "permissions": { "allow": ["Bash"] },
                "statusLine": { "type": "command", "command": "echo hi" }
            }),
        );

        let state = AppState::try_new_with_startup_recovery().expect("create startup state");

        let snippet = state
            .db
            .get_config_snippet("claude")
            .expect("read snippet")
            .expect("claude snippet should be seeded");
        let parsed: serde_json::Value = serde_json::from_str(&snippet).expect("snippet json");
        assert!(parsed.get("permissions").is_some());
        assert!(parsed.get("statusLine").is_some());
        // 鉴权/endpoint 字段不进通用配置
        assert!(parsed
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .is_none());
        // 非破坏式：default 供应商仍保留完整 settings
        let provider = state
            .db
            .get_provider_by_id("default", "claude")
            .expect("read provider")
            .expect("default provider");
        assert_eq!(
            provider.settings_config["env"]["ANTHROPIC_API_KEY"],
            json!("live-key")
        );
    }

    #[test]
    #[serial(home_settings)]
    fn startup_seeds_codex_common_config_snippet_from_live() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        write_json(
            crate::codex_config::get_codex_auth_path(),
            json!({ "OPENAI_API_KEY": "live-codex-key" }),
        );
        write_text(
            crate::codex_config::get_codex_config_path(),
            "model_provider = \"legacy\"\nmodel = \"gpt-4\"\n\n[tui]\ntheme = \"dark\"\n\n[model_providers.legacy]\nbase_url = \"https://api.example.com/v1\"\nwire_api = \"responses\"\n",
        );

        let state = AppState::try_new_with_startup_recovery().expect("create startup state");

        let snippet = state
            .db
            .get_config_snippet("codex")
            .expect("read snippet")
            .expect("codex snippet should be seeded");
        assert!(snippet.contains("[tui]"));
        assert!(snippet.contains("theme"));
        // 供应商专属字段被剥离
        assert!(!snippet.contains("model_providers"));
        assert!(!snippet.contains("model ="));
    }

    #[test]
    #[serial(home_settings)]
    fn startup_seeds_gemini_common_config_snippet_from_live() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        write_text(
            crate::gemini_config::get_gemini_env_path(),
            "GEMINI_API_KEY=live-key\nGOOGLE_GEMINI_BASE_URL=https://x\nHTTPS_PROXY=http://proxy.local:8080\n",
        );

        let state = AppState::try_new_with_startup_recovery().expect("create startup state");

        let snippet = state
            .db
            .get_config_snippet("gemini")
            .expect("read snippet")
            .expect("gemini snippet should be seeded");
        let parsed: serde_json::Value = serde_json::from_str(&snippet).expect("snippet json");
        assert_eq!(
            parsed.get("HTTPS_PROXY"),
            Some(&json!("http://proxy.local:8080"))
        );
        // 鉴权/endpoint 字段不进通用配置
        assert!(parsed.get("GEMINI_API_KEY").is_none());
        assert!(parsed.get("GOOGLE_GEMINI_BASE_URL").is_none());

        // 非破坏式：default 供应商仍保留完整 settings
        let provider = state
            .db
            .get_provider_by_id("default", "gemini")
            .expect("read provider")
            .expect("default provider");
        assert_eq!(
            provider.settings_config["env"]["GEMINI_API_KEY"],
            json!("live-key")
        );
    }

    #[test]
    #[serial(home_settings)]
    fn startup_does_not_overwrite_existing_common_config_snippet() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        write_json(
            crate::config::get_claude_settings_path(),
            json!({ "permissions": { "allow": ["Bash"] } }),
        );

        // 首次启动播种，然后用户改成自定义值
        {
            let state = AppState::try_new_with_startup_recovery().expect("first startup");
            assert!(state.db.get_config_snippet("claude").unwrap().is_some());
            state
                .db
                .set_config_snippet("claude", Some("{\"custom\":true}".to_string()))
                .expect("override snippet");
        }

        // 再次启动不得覆盖已有片段
        let state = AppState::try_new_with_startup_recovery().expect("second startup");
        let snippet = state.db.get_config_snippet("claude").unwrap().unwrap();
        assert!(snippet.contains("custom"));
    }

    #[test]
    #[serial(home_settings)]
    fn startup_skips_seeding_when_no_common_fields() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        // 仅含鉴权/endpoint，无可提取的共享字段
        write_json(
            crate::config::get_claude_settings_path(),
            json!({ "env": { "ANTHROPIC_API_KEY": "k", "ANTHROPIC_BASE_URL": "u" } }),
        );

        let state = AppState::try_new_with_startup_recovery().expect("startup");
        assert!(state
            .db
            .get_config_snippet("claude")
            .expect("read")
            .is_none());
        assert!(!state
            .db
            .is_config_snippet_cleared("claude")
            .expect("cleared flag"));
    }

    #[test]
    #[serial(home_settings)]
    fn startup_does_not_reseed_after_user_clears_snippet() {
        let temp_home = TempDir::new().expect("create temp home");
        let _env = TestEnvGuard::isolated(temp_home.path());

        write_json(
            crate::config::get_claude_settings_path(),
            json!({ "permissions": { "allow": ["Bash"] } }),
        );

        // 首次启动播种，随后用户清空
        {
            let state = AppState::try_new_with_startup_recovery().expect("first startup");
            assert!(state.db.get_config_snippet("claude").unwrap().is_some());

            crate::services::provider::ProviderService::clear_common_config_snippet(
                &state,
                crate::app_config::AppType::Claude,
            )
            .expect("clear snippet");

            assert!(state.db.is_config_snippet_cleared("claude").unwrap());
            assert!(state.db.get_config_snippet("claude").unwrap().is_none());
        }

        // 再次启动不得重新播种（用户已显式清空）
        let state = AppState::try_new_with_startup_recovery().expect("second startup");
        assert!(state.db.get_config_snippet("claude").unwrap().is_none());
        assert!(state.db.is_config_snippet_cleared("claude").unwrap());
    }
}
