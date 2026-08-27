use indexmap::IndexMap;

use crate::app_config::AppType;
use crate::config::write_text_file;
use crate::error::AppError;
use crate::prompt::Prompt;
use crate::prompt_files::prompt_file_path;
use crate::services::pi_prompt_files::PiAgentsFileGuard;
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
        if matches!(app, AppType::Pi) {
            return get_pi_prompts(state);
        }
        state.db.get_prompts(app.as_str())
    }

    pub fn upsert_prompt(
        state: &AppState,
        app: AppType,
        id: &str,
        prompt: Prompt,
    ) -> Result<(), AppError> {
        if matches!(app, AppType::Pi) {
            return upsert_pi_prompt(state, id, prompt);
        }

        let is_enabled = prompt.enabled;

        state.db.save_prompt(app.as_str(), &prompt)?;

        if is_enabled {
            let target_path = prompt_file_path(&app)?;
            write_text_file(&target_path, &prompt.content)?;
        }

        Ok(())
    }

    pub fn delete_prompt(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        if matches!(app, AppType::Pi) {
            return delete_pi_prompt(state, id);
        }
        let prompts = Self::get_prompts(state, app.clone())?;

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
        let prompts = Self::get_prompts(state, app.clone())?;
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

        // Pi persists `enabled = false` in SQLite because the native AGENTS.md
        // file is the source of truth. Hydrate that live state before editing,
        // otherwise an ordinary metadata/content edit would look like a
        // disable request and remove the active file.
        let prompts = if matches!(app, AppType::Pi) {
            get_pi_prompts(state)?
        } else {
            state.db.get_prompts(app.as_str())?
        };
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
        if matches!(app, AppType::Pi) {
            if old_prompt_id == prompt.id {
                upsert_pi_prompt(state, &prompt.id, prompt.clone())?;
            } else {
                rename_pi_prompt(state, &old_prompt_id, prompt.clone())?;
            }
        } else {
            state.db.save_prompt(app.as_str(), &prompt)?;
            if old_prompt_id != prompt.id {
                state.db.delete_prompt(app.as_str(), &old_prompt_id)?;
            }
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
        if matches!(app, AppType::Pi) {
            return enable_pi_prompt(state, id);
        }

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
        if matches!(app, AppType::Pi) {
            let mut prompt = get_pi_prompts(state)?
                .get(id)
                .cloned()
                .ok_or_else(|| AppError::InvalidInput(format!("提示词 {id} 不存在")))?;
            if !prompt.enabled {
                return Err(AppError::InvalidInput(format!("提示词 {id} 未激活")));
            }
            prompt.enabled = false;
            return upsert_pi_prompt(state, id, prompt);
        }

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
        let content = if matches!(app, AppType::Pi) {
            PiAgentsFileGuard::acquire()?
                .read()?
                .content
                .ok_or_else(|| AppError::Message("提示词文件不存在".to_string()))?
        } else {
            let file_path = prompt_file_path(&app)?;
            if !file_path.exists() {
                return Err(AppError::Message("提示词文件不存在".to_string()));
            }
            std::fs::read_to_string(&file_path).map_err(|e| AppError::io(&file_path, e))?
        };
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
        if matches!(app, AppType::Pi) {
            return Ok(PiAgentsFileGuard::acquire()?.read()?.content);
        }
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
            if matches!(app, AppType::Pi) {
                continue;
            }
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

fn pi_active_prompt_id(
    prompts: &IndexMap<String, Prompt>,
    live_content: Option<&str>,
) -> Option<String> {
    let live_content = live_content?;
    prompts
        .iter()
        .find(|(_, prompt)| prompt.content == live_content)
        .map(|(id, _)| id.clone())
}

fn unique_pi_backup_id(prompts: &IndexMap<String, Prompt>, timestamp: i64) -> String {
    let base = format!("backup-{timestamp}");
    if !prompts.contains_key(&base) {
        return base;
    }
    for suffix in 2_u64.. {
        let candidate = format!("{base}-{suffix}");
        if !prompts.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!("the backup suffix space is finite only after u64 exhaustion")
}

fn get_pi_prompts(state: &AppState) -> Result<IndexMap<String, Prompt>, AppError> {
    let guard = PiAgentsFileGuard::acquire()?;
    let mut prompts = state.db.get_prompts(AppType::Pi.as_str())?;
    let snapshot = guard.read()?;
    let active_id = pi_active_prompt_id(&prompts, snapshot.content.as_deref());

    for (id, prompt) in &mut prompts {
        prompt.enabled = active_id.as_ref() == Some(id);
    }
    Ok(prompts)
}

fn upsert_pi_prompt(state: &AppState, id: &str, prompt: Prompt) -> Result<(), AppError> {
    if prompt.id != id {
        return Err(AppError::InvalidInput(
            "Pi prompt id does not match the requested id".to_string(),
        ));
    }

    let guard = PiAgentsFileGuard::acquire()?;
    let prompts = state.db.get_prompts(AppType::Pi.as_str())?;
    let snapshot = guard.read()?;
    let was_active =
        pi_active_prompt_id(&prompts, snapshot.content.as_deref()).as_deref() == Some(id);
    let previous = prompts.get(id).cloned();
    let requested_active = prompt.enabled;
    let mut stored = prompt;
    stored.enabled = false;

    if requested_active && !was_active {
        return Err(AppError::Conflict(
            "Pi AGENTS.md changed outside CC Switch; reload before editing it".to_string(),
        ));
    }

    persist_pi_prompt_with_native_update(state, id, &stored, previous.as_ref(), || {
        if requested_active {
            guard.replace(&snapshot.revision, &stored.content)
        } else if was_active {
            guard.delete(&snapshot.revision)
        } else {
            Ok(())
        }
    })
}

fn persist_pi_prompt_with_native_update(
    state: &AppState,
    id: &str,
    stored: &Prompt,
    previous: Option<&Prompt>,
    update_native: impl FnOnce() -> Result<(), AppError>,
) -> Result<(), AppError> {
    state.db.save_prompt(AppType::Pi.as_str(), stored)?;
    if let Err(native_error) = update_native() {
        let rollback = match previous {
            Some(previous) => state.db.save_prompt(AppType::Pi.as_str(), previous),
            None => state.db.delete_prompt(AppType::Pi.as_str(), id),
        };
        if let Err(rollback_error) = rollback {
            return Err(AppError::Message(format!(
                "Pi prompt update failed ({native_error}); database rollback also failed: {rollback_error}"
            )));
        }
        return Err(native_error);
    }
    Ok(())
}

fn rename_pi_prompt(state: &AppState, old_id: &str, prompt: Prompt) -> Result<(), AppError> {
    let guard = PiAgentsFileGuard::acquire()?;
    let prompts = state.db.get_prompts(AppType::Pi.as_str())?;
    let previous = prompts
        .get(old_id)
        .cloned()
        .ok_or_else(|| AppError::InvalidInput(format!("提示词 {old_id} 不存在")))?;
    let snapshot = guard.read()?;
    let was_active =
        pi_active_prompt_id(&prompts, snapshot.content.as_deref()).as_deref() == Some(old_id);
    if prompt.enabled && !was_active {
        return Err(AppError::Conflict(
            "Pi AGENTS.md changed outside CC Switch; reload before editing it".to_string(),
        ));
    }

    let mut stored = prompt;
    stored.enabled = false;
    state.db.save_prompt(AppType::Pi.as_str(), &stored)?;
    if let Err(error) = state.db.delete_prompt(AppType::Pi.as_str(), old_id) {
        let _ = state.db.delete_prompt(AppType::Pi.as_str(), &stored.id);
        return Err(error);
    }

    let native_result = if was_active {
        guard.replace(&snapshot.revision, &stored.content)
    } else {
        Ok(())
    };
    if let Err(native_error) = native_result {
        let restore_old = state.db.save_prompt(AppType::Pi.as_str(), &previous);
        let remove_new = state.db.delete_prompt(AppType::Pi.as_str(), &stored.id);
        if let Err(rollback_error) = restore_old.and(remove_new) {
            return Err(AppError::Message(format!(
                "Pi prompt rename failed ({native_error}); database rollback also failed: {rollback_error}"
            )));
        }
        return Err(native_error);
    }
    Ok(())
}

fn enable_pi_prompt(state: &AppState, id: &str) -> Result<(), AppError> {
    let guard = PiAgentsFileGuard::acquire()?;
    let prompts = state.db.get_prompts(AppType::Pi.as_str())?;
    let target = prompts
        .get(id)
        .cloned()
        .ok_or_else(|| AppError::InvalidInput(format!("提示词 {id} 不存在")))?;
    let snapshot = guard.read()?;

    if let Some(content) = snapshot.content.as_ref() {
        let already_saved = prompts.values().any(|prompt| prompt.content == *content);
        if !content.trim().is_empty() && !already_saved {
            let timestamp = get_unix_timestamp()?;
            let backup = Prompt {
                id: unique_pi_backup_id(&prompts, timestamp),
                name: format!(
                    "原始提示词 {}",
                    chrono::Local::now().format("%Y-%m-%d %H:%M")
                ),
                content: content.clone(),
                description: Some("自动备份的原始提示词".to_string()),
                enabled: false,
                created_at: Some(timestamp),
                updated_at: Some(timestamp),
            };
            state.db.save_prompt(AppType::Pi.as_str(), &backup)?;
        }
    }

    guard.replace(&snapshot.revision, &target.content)
}

fn delete_pi_prompt(state: &AppState, id: &str) -> Result<(), AppError> {
    let guard = PiAgentsFileGuard::acquire()?;
    let prompts = state.db.get_prompts(AppType::Pi.as_str())?;
    let snapshot = guard.read()?;
    if pi_active_prompt_id(&prompts, snapshot.content.as_deref()).as_deref() == Some(id) {
        return Err(AppError::InvalidInput("无法删除已启用的提示词".to_string()));
    }
    state.db.delete_prompt(AppType::Pi.as_str(), id)?;
    Ok(())
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

#[cfg(test)]
mod pi_prompt_tests {
    use super::*;
    use crate::database::Database;
    use crate::pi_config::test_support::TestAgentDir;
    use serial_test::serial;
    use std::sync::Arc;

    fn prompt(enabled: bool) -> Prompt {
        Prompt {
            id: "test-prompt".to_string(),
            name: "Test prompt".to_string(),
            content: "managed content".to_string(),
            description: None,
            enabled,
            created_at: Some(1),
            updated_at: Some(1),
        }
    }

    #[test]
    #[serial]
    fn pi_active_prompt_is_derived_from_agents_file() {
        let _agent = TestAgentDir::new();
        let state = AppState::new(Arc::new(
            Database::memory().expect("create in-memory database"),
        ));
        state
            .db
            .save_prompt(AppType::Pi.as_str(), &prompt(true))
            .expect("save prompt");

        let saved = PromptService::get_prompts(&state, AppType::Pi).expect("load prompts");
        assert!(!saved["test-prompt"].enabled);

        let path = prompt_file_path(&AppType::Pi).expect("prompt path");
        write_text_file(&path, "managed content").expect("write AGENTS.md");
        let active = PromptService::get_prompts(&state, AppType::Pi).expect("load prompts");
        assert!(active["test-prompt"].enabled);

        write_text_file(&path, "external edit").expect("edit AGENTS.md externally");
        let drifted = PromptService::get_prompts(&state, AppType::Pi).expect("load prompts");
        assert!(!drifted["test-prompt"].enabled);
        assert!(
            PromptService::upsert_prompt(&state, AppType::Pi, "test-prompt", prompt(true),)
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read AGENTS.md"),
            "external edit"
        );

        write_text_file(&path, "managed content").expect("restore AGENTS.md");
        PromptService::upsert_prompt(&state, AppType::Pi, "test-prompt", prompt(false))
            .expect("disable prompt");
        assert!(!path.exists());
    }

    #[test]
    #[serial]
    fn generic_prompt_projection_does_not_rewrite_pi_agents_file() {
        let _agent = TestAgentDir::new();
        let state = AppState::new(Arc::new(
            Database::memory().expect("create in-memory database"),
        ));
        state
            .db
            .save_prompt(AppType::Pi.as_str(), &prompt(false))
            .expect("save Pi prompt");

        let path = prompt_file_path(&AppType::Pi).expect("prompt path");
        write_text_file(&path, "native instructions").expect("write AGENTS.md");

        PromptService::sync_all_active_to_live_best_effort(&state)
            .expect("sync prompts without projecting Pi");

        assert_eq!(
            std::fs::read_to_string(path).expect("read AGENTS.md"),
            "native instructions"
        );
    }

    #[test]
    #[serial]
    fn editing_an_inactive_duplicate_pi_prompt_preserves_agents_file() {
        let _agent = TestAgentDir::new();
        let state = AppState::new(Arc::new(
            Database::memory().expect("create in-memory database"),
        ));
        let first = prompt(false);
        let mut duplicate = first.clone();
        duplicate.id = "duplicate-prompt".to_string();
        duplicate.name = "Duplicate prompt".to_string();
        duplicate.created_at = Some(2);
        state
            .db
            .save_prompt(AppType::Pi.as_str(), &first)
            .expect("save first prompt");
        state
            .db
            .save_prompt(AppType::Pi.as_str(), &duplicate)
            .expect("save duplicate prompt");
        let path = prompt_file_path(&AppType::Pi).expect("prompt path");
        write_text_file(&path, "managed content").expect("write AGENTS.md");

        let hydrated = PromptService::get_prompts(&state, AppType::Pi).expect("load prompts");
        assert!(hydrated["test-prompt"].enabled);
        assert!(!hydrated["duplicate-prompt"].enabled);

        duplicate.content = "edited duplicate".to_string();
        PromptService::upsert_prompt(&state, AppType::Pi, "duplicate-prompt", duplicate)
            .expect("edit inactive duplicate");

        assert_eq!(
            std::fs::read_to_string(&path).expect("read AGENTS.md"),
            "managed content"
        );
    }

    #[test]
    #[serial]
    fn generic_updates_preserve_the_active_pi_prompt_state() {
        let _agent = TestAgentDir::new();
        let state = AppState::new(Arc::new(
            Database::memory().expect("create in-memory database"),
        ));
        state
            .db
            .save_prompt(AppType::Pi.as_str(), &prompt(false))
            .expect("save prompt");
        let path = prompt_file_path(&AppType::Pi).expect("prompt path");
        write_text_file(&path, "managed content").expect("activate prompt");

        PromptService::update_prompt(
            &state,
            AppType::Pi,
            "test-prompt",
            "test-prompt",
            "Edited prompt",
            None,
            Some("edited content".to_string()),
        )
        .expect("edit active prompt content");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read active prompt"),
            "edited content"
        );

        PromptService::update_prompt_metadata(
            &state,
            AppType::Pi,
            "test-prompt",
            "test-prompt",
            "Renamed display label",
            Some("metadata only".to_string()),
        )
        .expect("edit active prompt metadata");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read active prompt"),
            "edited content"
        );
        assert!(
            PromptService::get_prompts(&state, AppType::Pi).expect("hydrate prompts")
                ["test-prompt"]
                .enabled
        );
    }

    #[test]
    fn pi_backup_ids_do_not_replace_an_existing_same_second_backup() {
        let mut prompts = IndexMap::new();
        let mut first = prompt(false);
        first.id = "backup-42".to_string();
        prompts.insert(first.id.clone(), first);
        let mut second = prompt(false);
        second.id = "backup-42-2".to_string();
        prompts.insert(second.id.clone(), second);

        assert_eq!(unique_pi_backup_id(&prompts, 42), "backup-42-3");
    }
}
