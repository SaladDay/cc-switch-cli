use indexmap::IndexMap;

use crate::app_config::AppType;
use crate::config::write_text_file;
use crate::error::AppError;
use crate::prompt::Prompt;
use crate::prompt_files::prompt_file_path;
use crate::store::AppState;

fn get_unix_timestamp() -> Result<i64, AppError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .map_err(|e| AppError::Message(format!("Failed to get system time: {e}")))
}

pub struct PromptService;

impl PromptService {
    pub fn validate_prompt_id(id: &str) -> Result<(), AppError> {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            return Err(AppError::InvalidInput("提示词 ID 不能为空".to_string()));
        }

        if !trimmed
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(AppError::InvalidInput(
                "提示词 ID 只能包含字母、数字、点、下划线和连字符".to_string(),
            ));
        }

        Ok(())
    }

    pub fn generate_prompt_id(name: &str, existing_ids: &[String]) -> String {
        let mut base_id = name
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string();

        if base_id.is_empty() {
            base_id = "prompt".to_string();
        }

        if !existing_ids.contains(&base_id) {
            return base_id;
        }

        let mut counter = 1;
        loop {
            let candidate = format!("{base_id}-{counter}");
            if !existing_ids.contains(&candidate) {
                return candidate;
            }
            counter += 1;
        }
    }

    pub fn get_prompts(
        state: &AppState,
        app: AppType,
    ) -> Result<IndexMap<String, Prompt>, AppError> {
        state.db.get_prompts(app.as_str())
    }

    pub fn upsert_prompt(
        state: &AppState,
        app: AppType,
        _id: &str,
        prompt: Prompt,
    ) -> Result<(), AppError> {
        let is_enabled = prompt.enabled;

        state.db.save_prompt(app.as_str(), &prompt)?;

        if is_enabled {
            let target_path = prompt_file_path(&app)?;
            write_text_file(&target_path, &prompt.content)?;
        }

        Ok(())
    }

    pub fn delete_prompt(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        let prompts = state.db.get_prompts(app.as_str())?;

        if let Some(prompt) = prompts.get(id) {
            if prompt.enabled {
                return Err(AppError::InvalidInput("无法删除已启用的提示词".to_string()));
            }
        }

        state.db.delete_prompt(app.as_str(), id)?;
        Ok(())
    }

    pub fn rename_prompt(
        state: &AppState,
        app: AppType,
        id: &str,
        name: &str,
    ) -> Result<(), AppError> {
        let prompts = state.db.get_prompts(app.as_str())?;
        let Some(existing) = prompts.get(id) else {
            return Err(AppError::InvalidInput(format!("提示词 {id} 不存在")));
        };
        Self::update_prompt_metadata(state, app, id, id, name, existing.description.clone())?;
        Ok(())
    }

    pub fn update_prompt_metadata(
        state: &AppState,
        app: AppType,
        old_id: &str,
        new_id: &str,
        name: &str,
        description: Option<String>,
    ) -> Result<Prompt, AppError> {
        Self::update_prompt(state, app, old_id, new_id, name, description, None)
    }

    pub fn update_prompt(
        state: &AppState,
        app: AppType,
        old_id: &str,
        new_id: &str,
        name: &str,
        description: Option<String>,
        content: Option<String>,
    ) -> Result<Prompt, AppError> {
        let new_id = new_id.trim();
        Self::validate_prompt_id(new_id)?;

        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(AppError::InvalidInput("提示词名称不能为空".to_string()));
        }

        let prompts = state.db.get_prompts(app.as_str())?;
        if old_id != new_id && prompts.contains_key(new_id) {
            return Err(AppError::InvalidInput(format!("提示词 ID {new_id} 已存在")));
        }

        let Some(existing) = prompts.get(old_id) else {
            return Err(AppError::InvalidInput(format!("提示词 {old_id} 不存在")));
        };

        let mut prompt = existing.clone();
        let old_prompt_id = prompt.id.clone();
        prompt.id = new_id.to_string();
        prompt.name = trimmed.to_string();
        prompt.description = description.and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });
        if let Some(content) = content {
            prompt.content = content.trim_end().to_string();
        }
        prompt.updated_at = Some(get_unix_timestamp()?);
        state.db.save_prompt(app.as_str(), &prompt)?;
        if old_prompt_id != prompt.id {
            state.db.delete_prompt(app.as_str(), &old_prompt_id)?;
        }

        Ok(prompt)
    }

    pub fn create_prompt(
        state: &AppState,
        app: AppType,
        name: &str,
        content: &str,
    ) -> Result<Prompt, AppError> {
        Self::create_prompt_with_id(state, app, None, name, None, content)
    }

    pub fn create_prompt_with_id(
        state: &AppState,
        app: AppType,
        id: Option<&str>,
        name: &str,
        description: Option<&str>,
        content: &str,
    ) -> Result<Prompt, AppError> {
        let trimmed_name = name.trim();
        if trimmed_name.is_empty() {
            return Err(AppError::InvalidInput("提示词名称不能为空".to_string()));
        }

        let existing_ids = Self::get_prompts(state, app.clone())?
            .into_keys()
            .collect::<Vec<_>>();
        let id = match id {
            Some(id) if !id.trim().is_empty() => id.trim().to_string(),
            _ => Self::generate_prompt_id(trimmed_name, &existing_ids),
        };
        Self::validate_prompt_id(&id)?;
        if existing_ids.contains(&id) {
            return Err(AppError::InvalidInput(format!("提示词 ID {id} 已存在")));
        }

        let timestamp = get_unix_timestamp()?;
        let prompt = Prompt {
            id: id.clone(),
            name: trimmed_name.to_string(),
            content: content.trim_end().to_string(),
            description: description.and_then(|value| {
                let trimmed = value.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }),
            enabled: false,
            created_at: Some(timestamp),
            updated_at: Some(timestamp),
        };

        Self::upsert_prompt(state, app, &id, prompt.clone())?;
        Ok(prompt)
    }

    pub fn enable_prompt(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        let app_key = app.as_str();
        let target_path = prompt_file_path(&app)?;

        if target_path.exists() {
            if let Ok(live_content) = std::fs::read_to_string(&target_path) {
                if !live_content.trim().is_empty() {
                    let mut prompts = state.db.get_prompts(app_key)?;

                    if let Some((enabled_id, enabled_prompt)) = prompts
                        .iter_mut()
                        .find(|(_, prompt)| prompt.enabled)
                        .map(|(id, prompt)| (id.clone(), prompt))
                    {
                        enabled_prompt.content = live_content.clone();
                        enabled_prompt.updated_at = Some(get_unix_timestamp()?);
                        log::info!("回填 live 提示词内容到已启用项: {enabled_id}");
                        state.db.save_prompt(app_key, enabled_prompt)?;
                    } else {
                        let content_exists = prompts
                            .values()
                            .any(|prompt| prompt.content.trim() == live_content.trim());
                        if !content_exists {
                            let timestamp = get_unix_timestamp()?;
                            let backup_id = format!("backup-{timestamp}");
                            let backup_prompt = Prompt {
                                id: backup_id.clone(),
                                name: format!(
                                    "原始提示词 {}",
                                    chrono::Local::now().format("%Y-%m-%d %H:%M")
                                ),
                                content: live_content,
                                description: Some("自动备份的原始提示词".to_string()),
                                enabled: false,
                                created_at: Some(timestamp),
                                updated_at: Some(timestamp),
                            };
                            log::info!("回填 live 提示词内容，创建备份: {backup_id}");
                            state.db.save_prompt(app_key, &backup_prompt)?;
                        }
                    }
                }
            }
        }

        let mut prompts = state.db.get_prompts(app_key)?;
        for prompt in prompts.values_mut() {
            prompt.enabled = false;
        }

        let Some(prompt) = prompts.get_mut(id) else {
            return Err(AppError::InvalidInput(format!("提示词 {id} 不存在")));
        };
        prompt.enabled = true;
        write_text_file(&target_path, &prompt.content)?;

        for prompt in prompts.values() {
            state.db.save_prompt(app_key, prompt)?;
        }

        Ok(())
    }

    pub fn disable_prompt(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        let app_key = app.as_str();
        let mut prompts = state.db.get_prompts(app_key)?;

        let Some(prompt) = prompts.get_mut(id) else {
            return Err(AppError::InvalidInput(format!("提示词 {} 不存在", id)));
        };
        if !prompt.enabled {
            return Err(AppError::InvalidInput(format!("提示词 {} 未激活", id)));
        }

        prompt.enabled = false;
        state.db.save_prompt(app_key, prompt)?;

        if !prompts.values().any(|prompt| prompt.enabled) {
            let target_path = prompt_file_path(&app)?;
            write_text_file(&target_path, "")?;
        }

        Ok(())
    }

    pub fn import_from_file(state: &AppState, app: AppType) -> Result<String, AppError> {
        let file_path = prompt_file_path(&app)?;

        if !file_path.exists() {
            return Err(AppError::Message("提示词文件不存在".to_string()));
        }

        let content =
            std::fs::read_to_string(&file_path).map_err(|e| AppError::io(&file_path, e))?;
        let timestamp = get_unix_timestamp()?;

        let id = format!("imported-{timestamp}");
        let prompt = Prompt {
            id: id.clone(),
            name: format!(
                "导入的提示词 {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M")
            ),
            content,
            description: Some("从现有配置文件导入".to_string()),
            enabled: false,
            created_at: Some(timestamp),
            updated_at: Some(timestamp),
        };

        Self::upsert_prompt(state, app, &id, prompt)?;
        Ok(id)
    }

    pub fn get_current_file_content(app: AppType) -> Result<Option<String>, AppError> {
        let file_path = prompt_file_path(&app)?;
        if !file_path.exists() {
            return Ok(None);
        }
        let content =
            std::fs::read_to_string(&file_path).map_err(|e| AppError::io(&file_path, e))?;
        Ok(Some(content))
    }

    pub fn sync_all_active_to_live_best_effort(state: &AppState) -> Result<(), AppError> {
        let mut active_prompts = Vec::new();

        for app in AppType::all() {
            let prompts = state.db.get_prompts(app.as_str())?;
            if let Some(prompt) = select_active_prompt(&prompts) {
                active_prompts.push((app, prompt.content));
            }
        }

        for (app, content) in active_prompts {
            if !crate::sync_policy::should_sync_live(&app) {
                continue;
            }

            let target_path = match prompt_file_path(&app) {
                Ok(path) => path,
                Err(err) => {
                    log::warn!("同步 {app} 提示词 live 文件时解析路径失败: {err}");
                    continue;
                }
            };

            if let Err(err) = write_text_file(&target_path, &content) {
                log::warn!("同步 {app} 提示词到 live 文件失败: {err}");
            }
        }

        Ok(())
    }
}

fn select_active_prompt(prompts: &IndexMap<String, Prompt>) -> Option<Prompt> {
    prompts
        .values()
        .filter(|prompt| prompt.enabled)
        .max_by_key(|prompt| {
            (
                prompt.updated_at.unwrap_or(prompt.created_at.unwrap_or(0)),
                prompt.created_at.unwrap_or(0),
                prompt.id.clone(),
            )
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::MultiAppConfig;
    use crate::database::Database;
    use crate::services::ProxyService;
    use serial_test::serial;
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::{Arc, RwLock};
    use tempfile::TempDir;

    struct TempHome {
        #[allow(dead_code)]
        dir: TempDir,
        _lock: crate::test_support::TestHomeSettingsLock,
        old_home: Option<OsString>,
        old_userprofile: Option<OsString>,
        old_config_dir: Option<OsString>,
    }

    impl TempHome {
        fn new() -> Self {
            let dir = TempDir::new().expect("create temp home");
            let lock = crate::test_support::lock_test_home_and_settings();
            let old_home = std::env::var_os("HOME");
            let old_userprofile = std::env::var_os("USERPROFILE");
            let old_config_dir = std::env::var_os("CC_SWITCH_CONFIG_DIR");

            std::env::set_var("HOME", dir.path());
            std::env::set_var("USERPROFILE", dir.path());
            std::env::set_var("CC_SWITCH_CONFIG_DIR", dir.path().join(".cc-switch"));
            crate::test_support::set_test_home_override(Some(dir.path()));
            crate::settings::reload_test_settings();

            Self {
                dir,
                _lock: lock,
                old_home,
                old_userprofile,
                old_config_dir,
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.old_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.old_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
            match &self.old_config_dir {
                Some(value) => std::env::set_var("CC_SWITCH_CONFIG_DIR", value),
                None => std::env::remove_var("CC_SWITCH_CONFIG_DIR"),
            }
            crate::test_support::set_test_home_override(self.old_home.as_deref().map(Path::new));
            crate::settings::reload_test_settings();
        }
    }

    fn state_with_config(config: MultiAppConfig) -> AppState {
        let db = Arc::new(Database::init().expect("init db"));
        AppState {
            proxy_service: ProxyService::new(db.clone()),
            db,
            config: RwLock::new(config),
        }
    }

    fn prompt(id: &str, content: &str, enabled: bool) -> Prompt {
        Prompt {
            id: id.to_string(),
            name: id.to_string(),
            content: content.to_string(),
            description: None,
            enabled,
            created_at: Some(1),
            updated_at: Some(1),
        }
    }

    #[test]
    #[serial]
    fn state_save_does_not_overwrite_db_prompts_from_stale_config() {
        let _home = TempHome::new();
        let mut stale_config = MultiAppConfig::default();
        stale_config
            .prompts
            .claude
            .prompts
            .insert("stale".to_string(), prompt("stale", "old", false));
        let state = state_with_config(stale_config);

        PromptService::upsert_prompt(
            &state,
            AppType::Claude,
            "fresh",
            prompt("fresh", "new", false),
        )
        .expect("save fresh prompt");
        state.save().expect("save stale config");

        let prompts = PromptService::get_prompts(&state, AppType::Claude).expect("load prompts");
        assert!(prompts.contains_key("fresh"));
        assert!(!prompts.contains_key("stale"));
    }

    #[test]
    #[serial]
    fn enable_prompt_backfills_live_to_previous_active_and_disable_clears_live() {
        let home = TempHome::new();
        let state = state_with_config(MultiAppConfig::default());
        let live_path =
            crate::prompt_files::prompt_file_path(&AppType::Claude).expect("claude prompt path");
        std::fs::create_dir_all(live_path.parent().expect("live parent"))
            .expect("create live parent");

        PromptService::upsert_prompt(
            &state,
            AppType::Claude,
            "old",
            prompt("old", "old stored", true),
        )
        .expect("save old prompt");
        PromptService::upsert_prompt(
            &state,
            AppType::Claude,
            "new",
            prompt("new", "new stored", false),
        )
        .expect("save new prompt");
        std::fs::write(&live_path, "edited live").expect("write live prompt");

        PromptService::enable_prompt(&state, AppType::Claude, "new").expect("enable new prompt");

        let prompts = PromptService::get_prompts(&state, AppType::Claude).expect("load prompts");
        assert_eq!(
            prompts.get("old").expect("old prompt").content,
            "edited live"
        );
        assert!(!prompts.get("old").expect("old prompt").enabled);
        assert!(prompts.get("new").expect("new prompt").enabled);
        assert_eq!(
            std::fs::read_to_string(&live_path).expect("read live prompt"),
            "new stored"
        );

        PromptService::disable_prompt(&state, AppType::Claude, "new")
            .expect("disable active prompt");
        assert_eq!(
            std::fs::read_to_string(&live_path).expect("read cleared live prompt"),
            ""
        );

        drop(home);
    }

    #[test]
    #[serial]
    fn enable_prompt_creates_backup_when_live_has_no_active_owner() {
        let _home = TempHome::new();
        let state = state_with_config(MultiAppConfig::default());
        let live_path =
            crate::prompt_files::prompt_file_path(&AppType::Claude).expect("claude prompt path");
        std::fs::create_dir_all(live_path.parent().expect("live parent"))
            .expect("create live parent");
        std::fs::write(&live_path, "manual live").expect("write live prompt");

        PromptService::upsert_prompt(
            &state,
            AppType::Claude,
            "target",
            prompt("target", "target content", false),
        )
        .expect("save target prompt");

        PromptService::enable_prompt(&state, AppType::Claude, "target")
            .expect("enable target prompt");

        let prompts = PromptService::get_prompts(&state, AppType::Claude).expect("load prompts");
        assert!(prompts.values().any(|prompt| {
            prompt.id.starts_with("backup-") && prompt.content == "manual live" && !prompt.enabled
        }));
        assert!(prompts.get("target").expect("target prompt").enabled);
    }

    #[test]
    #[serial]
    fn create_prompt_with_custom_id_and_description() {
        let _home = TempHome::new();
        let state = state_with_config(MultiAppConfig::default());

        let created = PromptService::create_prompt_with_id(
            &state,
            AppType::Claude,
            Some("custom.prompt"),
            "Custom Prompt",
            Some("  Custom description  "),
            "hello\n",
        )
        .expect("create custom prompt");

        assert_eq!(created.id, "custom.prompt");
        assert_eq!(created.name, "Custom Prompt");
        assert_eq!(created.description.as_deref(), Some("Custom description"));

        let prompts = PromptService::get_prompts(&state, AppType::Claude).expect("load prompts");
        let stored = prompts.get("custom.prompt").expect("stored prompt");
        assert_eq!(stored.content, "hello");
        assert_eq!(stored.description.as_deref(), Some("Custom description"));
    }

    #[test]
    #[serial]
    fn update_prompt_metadata_changes_id_and_preserves_content_and_enabled() {
        let _home = TempHome::new();
        let state = state_with_config(MultiAppConfig::default());

        PromptService::upsert_prompt(
            &state,
            AppType::Claude,
            "old-id",
            Prompt {
                id: "old-id".to_string(),
                name: "Old".to_string(),
                content: "body".to_string(),
                description: Some("old description".to_string()),
                enabled: true,
                created_at: Some(1),
                updated_at: Some(1),
            },
        )
        .expect("seed prompt");

        let updated = PromptService::update_prompt_metadata(
            &state,
            AppType::Claude,
            "old-id",
            "new-id",
            "New Name",
            Some("  new description  ".to_string()),
        )
        .expect("update metadata");

        assert_eq!(updated.id, "new-id");
        assert_eq!(updated.name, "New Name");
        assert_eq!(updated.content, "body");
        assert!(updated.enabled);
        assert_eq!(updated.description.as_deref(), Some("new description"));

        let prompts = PromptService::get_prompts(&state, AppType::Claude).expect("load prompts");
        assert!(!prompts.contains_key("old-id"));
        let stored = prompts.get("new-id").expect("new prompt id");
        assert_eq!(stored.content, "body");
        assert!(stored.enabled);
    }

    #[test]
    #[serial]
    fn update_prompt_metadata_rejects_id_conflict() {
        let _home = TempHome::new();
        let state = state_with_config(MultiAppConfig::default());

        PromptService::upsert_prompt(
            &state,
            AppType::Claude,
            "first",
            prompt("first", "one", false),
        )
        .expect("seed first prompt");
        PromptService::upsert_prompt(
            &state,
            AppType::Claude,
            "second",
            prompt("second", "two", false),
        )
        .expect("seed second prompt");

        let err = PromptService::update_prompt_metadata(
            &state,
            AppType::Claude,
            "first",
            "second",
            "First",
            None,
        )
        .expect_err("duplicate id should fail");

        assert!(err.to_string().contains("已存在"));
    }
}
