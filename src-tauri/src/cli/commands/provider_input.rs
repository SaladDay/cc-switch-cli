// Provider Add/Edit 命令的共享输入逻辑
// 提供可复用的交互式输入函数，供 add 和 edit 命令使用

use crate::app_config::AppType;
use crate::cli::i18n::texts;
use crate::error::AppError;
use crate::provider::Provider;
use colored::Colorize;
use inquire::{Confirm, Select, Text};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

const CODEX_OFFICIAL_BASE_URL: &str = "https://api.openai.com/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAddMode {
    Official,
    ThirdParty,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_official_settings_config_omits_auth_and_enables_openai_auth() {
        let cfg = build_codex_official_settings_config("gpt-4o", "chat");
        assert!(
            cfg.get("auth").is_none(),
            "official Codex provider should not require auth.json"
        );

        let toml_text = cfg
            .get("config")
            .and_then(Value::as_str)
            .expect("settings_config.config should be string");
        assert!(
            toml_text.contains("requires_openai_auth = true"),
            "official Codex provider should enable requires_openai_auth"
        );
        assert!(
            toml_text.contains("wire_api = \"responses\""),
            "official Codex provider should use responses wire API"
        );
    }
}

pub fn prompt_settings_config_for_add(
    app_type: &AppType,
    mode: ProviderAddMode,
) -> Result<Value, AppError> {
    match (app_type, mode) {
        (AppType::Claude, _) => prompt_claude_config(None),
        (AppType::Codex, ProviderAddMode::Official) => prompt_codex_official_config(None),
        (AppType::Codex, ProviderAddMode::ThirdParty) => prompt_codex_config(None),
        (AppType::Gemini, _) => prompt_gemini_config(None),
    }
}

fn build_codex_settings_config(
    api_key: Option<&str>,
    base_url: &str,
    model: &str,
    wire_api: &str,
) -> Value {
    let model = if model.trim().is_empty() {
        "gpt-5.2-codex"
    } else {
        model.trim()
    };
    let base_url = if base_url.trim().is_empty() {
        CODEX_OFFICIAL_BASE_URL
    } else {
        base_url.trim()
    };

    let config_toml = [
        format!("base_url = \"{}\"", base_url),
        format!("model = \"{}\"", model),
        "model_reasoning_effort = \"high\"".to_string(),
        "disable_response_storage = true".to_string(),
        format!("wire_api = \"{}\"", wire_api),
        "requires_openai_auth = true".to_string(),
    ]
    .join("\n");

    match api_key {
        Some(key) => json!({
            "auth": { "OPENAI_API_KEY": key.trim() },
            "config": config_toml
        }),
        None => json!({
            "config": config_toml
        }),
    }
}

fn build_codex_official_settings_config(model: &str, _wire_api: &str) -> Value {
    build_codex_settings_config(None, CODEX_OFFICIAL_BASE_URL, model, "responses")
}

/// 可选字段集合
#[derive(Default)]
pub struct OptionalFields {
    pub notes: Option<String>,
    pub icon: Option<String>,
    pub icon_color: Option<String>,
    pub sort_index: Option<usize>,
}

impl OptionalFields {
    /// 从现有 Provider 提取可选字段
    pub fn from_provider(provider: &Provider) -> Self {
        Self {
            notes: provider.notes.clone(),
            icon: provider.icon.clone(),
            icon_color: provider.icon_color.clone(),
            sort_index: provider.sort_index,
        }
    }
}

/// 生成唯一的 Provider ID
/// 基于名称转换为 kebab-case，如有冲突则追加数字后缀
pub fn generate_provider_id(name: &str, existing_ids: &[String]) -> String {
    // 转换为 kebab-case
    let base_id = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else if c.is_whitespace() {
                '-'
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    // 检查唯一性
    if !existing_ids.contains(&base_id) {
        return base_id;
    }

    // 追加数字后缀
    let mut counter = 1;
    loop {
        let candidate = format!("{}-{}", base_id, counter);
        if !existing_ids.contains(&candidate) {
            return candidate;
        }
        counter += 1;
    }
}

/// 收集基本字段：name, website_url
pub fn prompt_basic_fields(
    current: Option<&Provider>,
) -> Result<(String, Option<String>), AppError> {
    // 供应商名称：根据上下文选择方法
    let name = if let Some(provider) = current {
        // 编辑模式：预填充当前值
        Text::new(texts::provider_name_label())
            .with_initial_value(&provider.name)
            .with_help_message(texts::provider_name_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        // 新增模式：显示示例占位符
        Text::new(texts::provider_name_label())
            .with_placeholder("OpenAI")
            .with_help_message(texts::provider_name_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::InvalidInput(
            texts::provider_name_empty_error().to_string(),
        ));
    }

    // 官网 URL：同样处理
    let website_url = if let Some(provider) = current {
        let initial = provider.website_url.as_deref().unwrap_or("");
        Text::new(texts::website_url_label())
            .with_initial_value(initial)
            .with_help_message(texts::website_url_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::website_url_label())
            .with_placeholder("https://openai.com")
            .with_help_message(texts::website_url_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    let website_url = if website_url.trim().is_empty() {
        None
    } else {
        Some(website_url.trim().to_string())
    };

    Ok((name, website_url))
}

/// 根据应用类型收集 settings_config
pub fn prompt_settings_config(
    app_type: &AppType,
    current: Option<&Value>,
) -> Result<Value, AppError> {
    match app_type {
        AppType::Claude => prompt_claude_config(current),
        AppType::Codex => {
            let has_auth = current
                .and_then(|v| v.get("auth"))
                .and_then(|v| v.as_object())
                .map(|obj| !obj.is_empty())
                .unwrap_or(false);
            let current_config_str = current
                .and_then(|v| v.get("config"))
                .and_then(|c| c.as_str());
            let mut current_base_url: Option<String> = None;
            if let Some(cfg) = current_config_str {
                if let Ok(table) = toml::from_str::<toml::Table>(cfg) {
                    current_base_url = table
                        .get("base_url")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    if current_base_url.is_none() {
                        if let (Some(model_provider), Some(model_providers)) = (
                            table.get("model_provider").and_then(|v| v.as_str()),
                            table.get("model_providers").and_then(|v| v.as_table()),
                        ) {
                            current_base_url = model_providers
                                .get(model_provider)
                                .and_then(|v| v.as_table())
                                .and_then(|t| t.get("base_url"))
                                .and_then(|v| v.as_str())
                                .map(String::from);
                        }
                    }
                }
            }

            let is_openai_official_endpoint = current_base_url
                .as_deref()
                .map(|url| url.trim_start().starts_with("https://api.openai.com"))
                .unwrap_or(false);

            if !has_auth && is_openai_official_endpoint {
                prompt_codex_official_config(current)
            } else {
                prompt_codex_config(current)
            }
        }
        AppType::Gemini => prompt_gemini_config(current),
    }
}

/// 提示用户输入单个模型字段
///
/// # 参数
/// - `field_name`: 字段显示名称（如 "默认模型"）
/// - `env_key`: 环境变量键名（如 "ANTHROPIC_MODEL"）
/// - `placeholder`: 占位符示例值
/// - `current`: 当前配置（编辑模式）
///
/// # 返回
/// - `Some(value)`: 用户输入了值或需要保留现有值
/// - `None`: 用户留空且无现有值，不应写入配置
fn prompt_model_field(
    field_name: &str,
    env_key: &str,
    placeholder: &str,
    current: Option<&Value>,
) -> Result<Option<String>, AppError> {
    // 尝试提取现有值
    let existing_value = current
        .and_then(|v| v.get("env"))
        .and_then(|e| e.get(env_key))
        .and_then(|m| m.as_str());

    let input = if let Some(existing) = existing_value {
        // 编辑模式 - 有现有值：预填充
        Text::new(&format!("{}：", field_name))
            .with_initial_value(existing)
            .with_help_message(texts::model_default_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        // 新增模式或编辑模式无现有值：占位符
        Text::new(&format!("{}：", field_name))
            .with_placeholder(placeholder)
            .with_help_message(texts::model_default_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    let trimmed = input.trim();

    if trimmed.is_empty() {
        if existing_value.is_some() {
            // 编辑模式下清空 → 移除配置
            Ok(None)
        } else {
            // 新增模式或原本无值 → 不写入
            Ok(None)
        }
    } else {
        // 有输入值
        Ok(Some(trimmed.to_string()))
    }
}

/// Claude 配置输入
fn prompt_claude_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::config_claude_header().bright_cyan().bold());

    let api_key = if let Some(current_key) = current
        .and_then(|v| v.get("env"))
        .and_then(|e| e.get("ANTHROPIC_AUTH_TOKEN"))
        .and_then(|k| k.as_str())
        .filter(|s| !s.is_empty())
    {
        // 编辑模式：显示完整 API Key 供编辑
        Text::new(texts::api_key_label())
            .with_initial_value(current_key)
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        // 新增模式：占位符示例
        Text::new(texts::api_key_label())
            .with_placeholder("sk-ant-...")
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    let base_url = if let Some(current_url) = current
        .and_then(|v| v.get("env"))
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|u| u.as_str())
        .filter(|s| !s.is_empty())
    {
        Text::new(texts::base_url_label())
            .with_initial_value(current_url)
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::base_url_label())
            .with_placeholder(texts::base_url_placeholder())
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    // 询问是否配置模型
    let config_models = Confirm::new(texts::configure_model_names_prompt())
        .with_default(false)
        .with_help_message(texts::api_key_help())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    let mut env = serde_json::Map::new();
    env.insert("ANTHROPIC_AUTH_TOKEN".to_string(), json!(api_key.trim()));
    env.insert("ANTHROPIC_BASE_URL".to_string(), json!(base_url.trim()));

    if config_models {
        // 使用新的辅助函数处理四个模型字段
        let model = prompt_model_field(
            texts::model_default_label(),
            "ANTHROPIC_MODEL",
            texts::model_sonnet_placeholder(),
            current,
        )?;

        let haiku = prompt_model_field(
            texts::model_haiku_label(),
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            texts::model_haiku_placeholder(),
            current,
        )?;

        let sonnet = prompt_model_field(
            texts::model_sonnet_label(),
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            texts::model_sonnet_placeholder(),
            current,
        )?;

        let opus = prompt_model_field(
            texts::model_opus_label(),
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            texts::model_opus_placeholder(),
            current,
        )?;

        // 条件写入：只在值存在时写入配置
        if let Some(value) = model {
            env.insert("ANTHROPIC_MODEL".to_string(), json!(value));
        }
        if let Some(value) = haiku {
            env.insert("ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(), json!(value));
        }
        if let Some(value) = sonnet {
            env.insert("ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(), json!(value));
        }
        if let Some(value) = opus {
            env.insert("ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(), json!(value));
        }
    }

    Ok(json!({ "env": env }))
}

/// Codex 配置输入（第三方/自定义：需要 API Key）
fn prompt_codex_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::config_codex_header().bright_cyan().bold());

    // 从当前配置提取值
    let current_api_key = current
        .and_then(|v| v.get("auth"))
        .and_then(|a| a.get("OPENAI_API_KEY"))
        .and_then(|k| k.as_str())
        .filter(|s| !s.is_empty());

    let current_config_str = current
        .and_then(|v| v.get("config"))
        .and_then(|c| c.as_str());

    let mut current_base_url: Option<String> = None;
    let mut current_model: Option<String> = None;
    if let Some(cfg) = current_config_str {
        if let Ok(table) = toml::from_str::<toml::Table>(cfg) {
            current_base_url = table
                .get("base_url")
                .and_then(|v| v.as_str())
                .map(String::from);
            if current_base_url.is_none() {
                // Full upstream-style config: base_url lives under model_providers.<model_provider>.
                if let (Some(model_provider), Some(model_providers)) = (
                    table.get("model_provider").and_then(|v| v.as_str()),
                    table.get("model_providers").and_then(|v| v.as_table()),
                ) {
                    current_base_url = model_providers
                        .get(model_provider)
                        .and_then(|v| v.as_table())
                        .and_then(|t| t.get("base_url"))
                        .and_then(|v| v.as_str())
                        .map(String::from);
                }
            }
            current_model = table
                .get("model")
                .and_then(|v| v.as_str())
                .map(String::from);
        }
    }

    // 1. API Key（恢复：用于旧版本 Codex 兼容性）
    let api_key = if let Some(current_key) = current_api_key {
        Text::new(texts::openai_api_key_label())
            .with_initial_value(current_key)
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::openai_api_key_label())
            .with_placeholder("sk-...")
            .with_help_message(texts::api_key_help())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    // 2. Base URL
    let base_url = if let Some(current) = current_base_url.as_deref() {
        Text::new(&format!("{}:", texts::tui_label_base_url()))
            .with_initial_value(current)
            .with_help_message("API endpoint (e.g., https://api.openai.com/v1)")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(&format!("{}:", texts::tui_label_base_url()))
            .with_placeholder("https://api.openai.com/v1")
            .with_help_message("API endpoint")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };
    let base_url = base_url.trim().to_string();
    if base_url.is_empty() {
        return Err(AppError::InvalidInput(
            texts::base_url_empty_error().to_string(),
        ));
    }

    // 3. Model
    let model = if let Some(current) = current_model.as_deref() {
        Text::new(&format!("{}:", texts::model_label()))
            .with_initial_value(current)
            .with_help_message("Model name (e.g., gpt-5.2-codex, o3)")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(&format!("{}:", texts::model_label()))
            .with_placeholder("gpt-5.2-codex")
            .with_help_message("Model name")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    Ok(build_codex_settings_config(
        Some(api_key.trim()),
        &base_url,
        model.trim(),
        "responses",
    ))
}

/// Codex 配置输入（官方：不需要 API Key）
fn prompt_codex_official_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::config_codex_header().bright_cyan().bold());
    println!("\n{}", texts::tui_codex_official_no_api_key_tip().yellow());

    let current_config_str = current
        .and_then(|v| v.get("config"))
        .and_then(|c| c.as_str());

    let mut current_base_url: Option<String> = None;
    let mut current_model: Option<String> = None;
    if let Some(cfg) = current_config_str {
        if let Ok(table) = toml::from_str::<toml::Table>(cfg) {
            current_base_url = table
                .get("base_url")
                .and_then(|v| v.as_str())
                .map(String::from);
            if current_base_url.is_none() {
                if let (Some(model_provider), Some(model_providers)) = (
                    table.get("model_provider").and_then(|v| v.as_str()),
                    table.get("model_providers").and_then(|v| v.as_table()),
                ) {
                    current_base_url = model_providers
                        .get(model_provider)
                        .and_then(|v| v.as_table())
                        .and_then(|t| t.get("base_url"))
                        .and_then(|v| v.as_str())
                        .map(String::from);
                }
            }
            current_model = table
                .get("model")
                .and_then(|v| v.as_str())
                .map(String::from);
        }
    }

    let base_url = if let Some(current) = current_base_url.as_deref() {
        Text::new(&format!("{}:", texts::tui_label_base_url()))
            .with_initial_value(current)
            .with_help_message("API endpoint (e.g., https://api.openai.com/v1)")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(&format!("{}:", texts::tui_label_base_url()))
            .with_placeholder(CODEX_OFFICIAL_BASE_URL)
            .with_help_message("API endpoint")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    let model = if let Some(current) = current_model.as_deref() {
        Text::new(&format!("{}:", texts::model_label()))
            .with_initial_value(current)
            .with_help_message("Model name (e.g., gpt-5.2-codex, o3)")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(&format!("{}:", texts::model_label()))
            .with_placeholder("gpt-5.2-codex")
            .with_help_message("Model name")
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };

    Ok(build_codex_settings_config(
        None,
        base_url.trim(),
        model.trim(),
        "responses",
    ))
}

/// Gemini 配置输入（含认证类型选择）
fn prompt_gemini_config(current: Option<&Value>) -> Result<Value, AppError> {
    println!("\n{}", texts::config_gemini_header().bright_cyan().bold());

    // 检测当前认证类型
    let current_auth_type = detect_gemini_auth_type(current);
    let default_index = match current_auth_type.as_deref() {
        Some("oauth") => 0,
        _ => 1, // 默认 Generic API Key（包括 packycode 和 generic）
    };

    let auth_options = vec![texts::google_oauth_official(), texts::generic_api_key()];

    let auth_type = Select::new(texts::auth_type_label(), auth_options.clone())
        .with_starting_cursor(default_index)
        .with_help_message(texts::select_auth_method_help())
        .prompt()
        .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?;

    // Match using the translated strings
    let google_oauth = texts::google_oauth_official();

    if auth_type == google_oauth {
        println!("{}", texts::use_google_oauth_warning().yellow());
        Ok(json!({
            "env": {},
            "config": {}
        }))
    } else {
        // Generic API Key (统一处理所有 API Key 供应商，包括 PackyCode)
        let api_key = if let Some(current_key) = current
            .and_then(|v| v.get("env"))
            .and_then(|e| e.get("GEMINI_API_KEY"))
            .and_then(|k| k.as_str())
            .filter(|s| !s.is_empty())
        {
            Text::new(texts::gemini_api_key_label())
                .with_initial_value(current_key)
                .with_help_message(texts::generic_api_key_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        } else {
            Text::new(texts::gemini_api_key_label())
                .with_placeholder("AIza... or pk-...")
                .with_help_message(texts::generic_api_key_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        };

        let base_url = if let Some(current_url) = current
            .and_then(|v| v.get("env"))
            .and_then(|e| e.get("GOOGLE_GEMINI_BASE_URL"))
            .and_then(|u| u.as_str())
            .filter(|s| !s.is_empty())
        {
            Text::new(texts::gemini_base_url_label())
                .with_initial_value(current_url)
                .with_help_message(texts::gemini_base_url_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        } else {
            Text::new(texts::gemini_base_url_label())
                .with_placeholder(texts::gemini_base_url_placeholder())
                .with_help_message(texts::gemini_base_url_help())
                .prompt()
                .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
        };

        Ok(json!({
            "env": {
                "GEMINI_API_KEY": api_key.trim(),
                "GOOGLE_GEMINI_BASE_URL": base_url.trim()
            },
            "config": {}
        }))
    }
}

/// 收集可选字段
pub fn prompt_optional_fields(current: Option<&Provider>) -> Result<OptionalFields, AppError> {
    println!("\n{}", texts::optional_fields_config().bright_cyan().bold());

    let notes = if let Some(provider) = current {
        let initial = provider.notes.as_deref().unwrap_or("");
        Text::new(texts::notes_label())
            .with_initial_value(initial)
            .with_help_message(texts::notes_help_edit())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::notes_label())
            .with_placeholder(texts::notes_example_placeholder())
            .with_help_message(texts::notes_help_new())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };
    let notes = if notes.trim().is_empty() {
        None
    } else {
        Some(notes.trim().to_string())
    };

    let sort_index_str = if let Some(provider) = current {
        let initial = provider
            .sort_index
            .map(|i| i.to_string())
            .unwrap_or_default();
        Text::new(texts::sort_index_label())
            .with_initial_value(&initial)
            .with_help_message(texts::sort_index_help_edit())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    } else {
        Text::new(texts::sort_index_label())
            .with_placeholder(texts::sort_index_placeholder())
            .with_help_message(texts::sort_index_help_new())
            .prompt()
            .map_err(|e| AppError::Message(texts::input_failed_error(&e.to_string())))?
    };
    let sort_index =
        if sort_index_str.trim().is_empty() {
            None
        } else {
            Some(sort_index_str.trim().parse::<usize>().map_err(|_| {
                AppError::InvalidInput(texts::invalid_sort_index_number().to_string())
            })?)
        };

    Ok(OptionalFields {
        notes,
        icon: None,
        icon_color: None,
        sort_index,
    })
}

/// 显示供应商配置摘要
pub fn display_provider_summary(provider: &Provider, app_type: &AppType) {
    println!(
        "\n{}",
        texts::provider_config_summary().bright_green().bold()
    );
    println!("{}: {}", texts::id_label().bright_yellow(), provider.id);
    println!(
        "{}: {}",
        texts::provider_name_label().bright_yellow(),
        provider.name
    );

    if let Some(website) = &provider.website_url {
        println!("{}: {}", texts::website_label().bright_yellow(), website);
    }

    // 显示关键配置（不显示完整 API Key）
    println!("\n{}", texts::core_config_label().bright_cyan());
    match app_type {
        AppType::Claude => {
            if let Some(env) = provider.settings_config.get("env") {
                if let Some(api_key) = env.get("ANTHROPIC_AUTH_TOKEN").and_then(|v| v.as_str()) {
                    println!(
                        "  {}: {}",
                        texts::api_key_display_label(),
                        mask_api_key(api_key)
                    );
                }
                if let Some(base_url) = env.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) {
                    println!("  {}: {}", texts::base_url_display_label(), base_url);
                }
                if let Some(model) = env.get("ANTHROPIC_MODEL").and_then(|v| v.as_str()) {
                    println!("  {}: {}", texts::model_label(), model);
                }
            }
        }
        AppType::Codex => {
            if let Some(auth) = provider.settings_config.get("auth") {
                if let Some(api_key) = auth.get("OPENAI_API_KEY").and_then(|v| v.as_str()) {
                    println!(
                        "  {}: {}",
                        texts::api_key_display_label(),
                        mask_api_key(api_key)
                    );
                }
            }
            if let Some(config) = provider
                .settings_config
                .get("config")
                .and_then(|v| v.as_str())
            {
                println!("  {}", texts::config_toml_lines(config.lines().count()));
            }
        }
        AppType::Gemini => {
            if let Some(env) = provider.settings_config.get("env") {
                if let Some(api_key) = env.get("GEMINI_API_KEY").and_then(|v| v.as_str()) {
                    println!(
                        "  {}: {}",
                        texts::api_key_display_label(),
                        mask_api_key(api_key)
                    );
                }
                if let Some(base_url) = env
                    .get("GOOGLE_GEMINI_BASE_URL")
                    .or_else(|| env.get("BASE_URL"))
                    .and_then(|v| v.as_str())
                {
                    println!("  {}: {}", texts::base_url_display_label(), base_url);
                }
            }
        }
    }

    // 可选字段
    if provider.notes.is_some() || provider.sort_index.is_some() {
        println!("\n{}", texts::optional_fields_label().bright_cyan());
        if let Some(notes) = &provider.notes {
            println!("  {}: {}", texts::notes_label_colon(), notes);
        }
        if let Some(idx) = provider.sort_index {
            println!("  {}: {}", texts::sort_index_label_colon(), idx);
        }
    }

    println!("{}", texts::summary_divider().bright_green().bold());
}

/// 获取当前时间戳（秒）
pub fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// ========== 辅助函数 ==========
/// 检测 Gemini 当前的认证类型
fn detect_gemini_auth_type(value: Option<&Value>) -> Option<String> {
    if let Some(env) = value.and_then(|v| v.get("env")) {
        if env.get("GEMINI_API_KEY").is_some() {
            if env
                .get("GOOGLE_GEMINI_BASE_URL")
                .and_then(|v| v.as_str())
                .map(|s| s.contains("packycode"))
                .unwrap_or(false)
            {
                return Some("packycode".to_string());
            } else {
                return Some("generic".to_string());
            }
        }
    }
    // 如果没有 API Key，假设是 OAuth
    if value
        .and_then(|v| v.get("env"))
        .map(|v| v.as_object().map(|o| o.is_empty()).unwrap_or(true))
        .unwrap_or(true)
    {
        return Some("oauth".to_string());
    }
    None
}

/// 遮蔽 API Key 显示（用于摘要显示）
fn mask_api_key(key: &str) -> String {
    if key.len() <= 8 {
        return "***".to_string();
    }
    format!("{}...{}", &key[..4], &key[key.len() - 4..])
}
