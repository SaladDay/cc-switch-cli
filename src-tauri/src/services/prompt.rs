use std::collections::HashMap;

use crate::app_config::AppType;
use crate::config::write_text_file;
use crate::error::AppError;
use crate::prompt::Prompt;
use crate::prompt_files::prompt_file_path;
use crate::store::AppState;

pub struct PromptService;

impl PromptService {
    pub fn get_prompts(
        state: &AppState,
        app: AppType,
    ) -> Result<HashMap<String, Prompt>, AppError> {
        let cfg = state.config.read()?;
        let prompts = match app {
            AppType::Claude => &cfg.prompts.claude.prompts,
            AppType::Codex => &cfg.prompts.codex.prompts,
            AppType::Gemini => &cfg.prompts.gemini.prompts,
            AppType::OpenCode => &cfg.prompts.opencode.prompts,
            AppType::OpenClaw => &cfg.prompts.openclaw.prompts,
        };
        Ok(prompts.clone())
    }

    pub fn upsert_prompt(
        state: &AppState,
        app: AppType,
        id: &str,
        prompt: Prompt,
    ) -> Result<(), AppError> {
        // 检查是否为已启用的提示词
        let is_enabled = prompt.enabled;

        let mut cfg = state.config.write()?;
        let prompts = match app {
            AppType::Claude => &mut cfg.prompts.claude.prompts,
            AppType::Codex => &mut cfg.prompts.codex.prompts,
            AppType::Gemini => &mut cfg.prompts.gemini.prompts,
            AppType::OpenCode => &mut cfg.prompts.opencode.prompts,
            AppType::OpenClaw => &mut cfg.prompts.openclaw.prompts,
        };
        prompts.insert(id.to_string(), prompt.clone());
        drop(cfg);
        state.save()?;

        // 如果是已启用的提示词，同步更新到对应的文件
        if is_enabled {
            let target_path = prompt_file_path(&app)?;
            write_text_file(&target_path, &prompt.content)?;
        }

        Ok(())
    }

    pub fn delete_prompt(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        let mut cfg = state.config.write()?;
        let prompts = match app {
            AppType::Claude => &mut cfg.prompts.claude.prompts,
            AppType::Codex => &mut cfg.prompts.codex.prompts,
            AppType::Gemini => &mut cfg.prompts.gemini.prompts,
            AppType::OpenCode => &mut cfg.prompts.opencode.prompts,
            AppType::OpenClaw => &mut cfg.prompts.openclaw.prompts,
        };

        if let Some(prompt) = prompts.get(id) {
            if prompt.enabled {
                return Err(AppError::InvalidInput("无法删除已启用的提示词".to_string()));
            }
        }

        prompts.remove(id);
        drop(cfg);
        state.save()?;
        Ok(())
    }

    pub fn enable_prompt(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        // 回填当前 live 文件内容到已启用的提示词，或创建备份
        let target_path = prompt_file_path(&app)?;
        if target_path.exists() {
            if let Ok(live_content) = std::fs::read_to_string(&target_path) {
                if !live_content.trim().is_empty() {
                    let mut cfg = state.config.write()?;
                    let prompts = match app {
                        AppType::Claude => &mut cfg.prompts.claude.prompts,
                        AppType::Codex => &mut cfg.prompts.codex.prompts,
                        AppType::Gemini => &mut cfg.prompts.gemini.prompts,
                        AppType::OpenCode => &mut cfg.prompts.opencode.prompts,
                        AppType::OpenClaw => &mut cfg.prompts.openclaw.prompts,
                    };

                    // 尝试回填到当前已启用的提示词
                    if let Some((enabled_id, enabled_prompt)) = prompts
                        .iter_mut()
                        .find(|(_, p)| p.enabled)
                        .map(|(id, p)| (id.clone(), p))
                    {
                        let timestamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64;
                        enabled_prompt.content = live_content.clone();
                        enabled_prompt.updated_at = Some(timestamp);
                        log::info!("回填 live 提示词内容到已启用项: {enabled_id}");
                        drop(cfg); // 释放锁后保存，避免死锁
                        state.save()?; // 第一次保存：回填后立即持久化
                    } else {
                        // 没有已启用的提示词，则创建一次备份（避免重复备份）
                        let content_exists = prompts
                            .values()
                            .any(|p| p.content.trim() == live_content.trim());
                        if !content_exists {
                            let timestamp = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs() as i64;
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
                            prompts.insert(backup_id.clone(), backup_prompt);
                            log::info!("回填 live 提示词内容，创建备份: {backup_id}");
                            drop(cfg); // 释放锁后保存
                            state.save()?; // 第一次保存：回填后立即持久化
                        } else {
                            // 即使内容已存在，也无需重复备份；但不需要保存任何更改
                            drop(cfg);
                        }
                    }
                }
            }
        }

        // 启用目标提示词并写入文件
        let mut cfg = state.config.write()?;
        let prompts = match app {
            AppType::Claude => &mut cfg.prompts.claude.prompts,
            AppType::Codex => &mut cfg.prompts.codex.prompts,
            AppType::Gemini => &mut cfg.prompts.gemini.prompts,
            AppType::OpenCode => &mut cfg.prompts.opencode.prompts,
            AppType::OpenClaw => &mut cfg.prompts.openclaw.prompts,
        };

        for prompt in prompts.values_mut() {
            prompt.enabled = false;
        }

        if let Some(prompt) = prompts.get_mut(id) {
            prompt.enabled = true;
            write_text_file(&target_path, &prompt.content)?; // 原子写入
        } else {
            return Err(AppError::InvalidInput(format!("提示词 {id} 不存在")));
        }

        drop(cfg);
        state.save()?; // 第二次保存：启用目标提示词并写入文件后
        Ok(())
    }

    pub fn disable_prompt(state: &AppState, app: AppType, id: &str) -> Result<(), AppError> {
        let mut cfg = state.config.write()?;
        let prompts = match app {
            AppType::Claude => &mut cfg.prompts.claude.prompts,
            AppType::Codex => &mut cfg.prompts.codex.prompts,
            AppType::Gemini => &mut cfg.prompts.gemini.prompts,
            AppType::OpenCode => &mut cfg.prompts.opencode.prompts,
            AppType::OpenClaw => &mut cfg.prompts.openclaw.prompts,
        };

        // 验证提示词是否存在且已启用
        if let Some(prompt) = prompts.get_mut(id) {
            if !prompt.enabled {
                return Err(AppError::InvalidInput(format!("提示词 {} 未激活", id)));
            }
            prompt.enabled = false;
        } else {
            return Err(AppError::InvalidInput(format!("提示词 {} 不存在", id)));
        }

        drop(cfg);
        state.save()?;

        // 清空对应的实时文件
        let target_path = prompt_file_path(&app)?;
        write_text_file(&target_path, "")?;

        Ok(())
    }

    pub fn import_from_file(state: &AppState, app: AppType) -> Result<String, AppError> {
        let file_path = prompt_file_path(&app)?;

        if !file_path.exists() {
            return Err(AppError::Message("提示词文件不存在".to_string()));
        }

        let content =
            std::fs::read_to_string(&file_path).map_err(|e| AppError::io(&file_path, e))?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

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
}
