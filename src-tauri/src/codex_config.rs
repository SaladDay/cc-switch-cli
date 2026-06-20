use std::path::PathBuf;

use crate::config::{
    atomic_write, delete_file, home_dir, read_json_file, sanitize_provider_name, write_json_file,
    write_text_file,
};
use crate::error::AppError;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::Command;
use toml_edit::DocumentMut;

pub const CC_SWITCH_CODEX_MODEL_PROVIDER_ID: &str = "custom";
pub const CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME: &str = "cc-switch-model-catalog.json";
const CODEX_MODEL_CATALOG_TEMPLATE_SLUG: &str = "gpt-5.5";

/// Reserved built-in provider IDs from OpenAI Codex's config/model-provider
/// catalog. Keep in sync with Codex `RESERVED_MODEL_PROVIDER_IDS` and legacy
/// removed provider aliases.
const CODEX_RESERVED_MODEL_PROVIDER_IDS: &[&str] = &[
    "amazon-bedrock",
    "openai",
    "ollama",
    "lmstudio",
    "oss",
    "ollama-chat",
];

/// 获取 Codex 配置目录路径
pub fn get_codex_config_dir() -> PathBuf {
    if let Some(custom) = crate::settings::get_codex_override_dir() {
        return custom;
    }

    if let Some(dir) = std::env::var_os("CODEX_HOME") {
        let dir = PathBuf::from(dir);
        if !dir.as_os_str().is_empty() && !dir.to_string_lossy().trim().is_empty() && dir.is_dir() {
            return dir;
        }
    }

    home_dir().expect("无法获取用户主目录").join(".codex")
}

/// 获取 Codex auth.json 路径
pub fn get_codex_auth_path() -> PathBuf {
    get_codex_config_dir().join("auth.json")
}

/// 获取 Codex config.toml 路径
pub fn get_codex_config_path() -> PathBuf {
    get_codex_config_dir().join("config.toml")
}

pub fn get_codex_model_catalog_path() -> PathBuf {
    get_codex_config_dir().join(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME)
}

/// 获取 Codex 供应商配置文件路径
pub fn get_codex_provider_paths(
    provider_id: &str,
    provider_name: Option<&str>,
) -> (PathBuf, PathBuf) {
    let base_name = provider_name
        .map(sanitize_provider_name)
        .unwrap_or_else(|| sanitize_provider_name(provider_id));

    let auth_path = get_codex_config_dir().join(format!("auth-{base_name}.json"));
    let config_path = get_codex_config_dir().join(format!("config-{base_name}.toml"));

    (auth_path, config_path)
}

/// 删除 Codex 供应商配置文件
pub fn delete_codex_provider_config(
    provider_id: &str,
    provider_name: &str,
) -> Result<(), AppError> {
    let (auth_path, config_path) = get_codex_provider_paths(provider_id, Some(provider_name));

    delete_file(&auth_path).ok();
    delete_file(&config_path).ok();

    Ok(())
}

/// 原子写 Codex 的 `auth.json` 与 `config.toml`，在第二步失败时回滚第一步
pub fn write_codex_live_atomic(
    auth: &Value,
    config_text_opt: Option<&str>,
) -> Result<(), AppError> {
    write_codex_live_atomic_optional_auth(Some(auth), config_text_opt)
}

pub fn write_codex_live_atomic_optional_auth(
    auth: Option<&Value>,
    config_text_opt: Option<&str>,
) -> Result<(), AppError> {
    let auth_path = get_codex_auth_path();
    let config_path = get_codex_config_path();

    if let Some(parent) = auth_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    // 读取旧内容用于回滚
    let old_auth = if auth_path.exists() {
        Some(fs::read(&auth_path).map_err(|e| AppError::io(&auth_path, e))?)
    } else {
        None
    };
    let _old_config = if config_path.exists() {
        Some(fs::read(&config_path).map_err(|e| AppError::io(&config_path, e))?)
    } else {
        None
    };

    // 准备写入内容
    let cfg_text = match config_text_opt {
        Some(s) => s.to_string(),
        None => String::new(),
    };
    if !cfg_text.trim().is_empty() {
        toml::from_str::<toml::Table>(&cfg_text).map_err(|e| AppError::toml(&config_path, e))?;
    }

    // 第一步：写 auth.json
    if let Some(auth) = auth {
        write_json_file(&auth_path, auth)?;
    } else {
        delete_file(&auth_path)?;
    }

    // 第二步：写 config.toml（失败则回滚 auth.json）
    if let Err(e) = write_text_file(&config_path, &cfg_text) {
        // 回滚 auth.json
        if let Some(bytes) = old_auth {
            let _ = atomic_write(&auth_path, &bytes);
        } else {
            let _ = delete_file(&auth_path);
        }
        return Err(e);
    }

    Ok(())
}

/// 读取 `~/.codex/config.toml`，若不存在返回空字符串
pub fn read_codex_config_text() -> Result<String, AppError> {
    let path = get_codex_config_path();
    if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))
    } else {
        Ok(String::new())
    }
}

/// 对非空的 TOML 文本进行语法校验
pub fn validate_config_toml(text: &str) -> Result<(), AppError> {
    if text.trim().is_empty() {
        return Ok(());
    }
    toml::from_str::<toml::Table>(text)
        .map(|_| ())
        .map_err(|e| AppError::toml(Path::new("config.toml"), e))
}

/// Remove provider-specific Codex TOML keys and keep only shared/global settings.
///
/// This matches upstream "OpenAI Official" snapshot semantics where the official
/// provider does not persist a provider-local `base_url` / `model_provider`
/// section, but may still carry root-level shared settings.
pub fn strip_codex_provider_config_text(config_toml: &str) -> Result<String, AppError> {
    let config_toml = config_toml.trim();
    if config_toml.is_empty() {
        return Ok(String::new());
    }

    let mut doc = config_toml
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| AppError::Config(format!("TOML parse error: {e}")))?;
    let root = doc.as_table_mut();
    root.remove("model");
    root.remove("model_provider");
    root.remove("base_url");
    root.remove("model_providers");

    let mut cleaned = String::new();
    let mut blank_run = 0usize;
    for line in doc.to_string().lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                cleaned.push('\n');
            }
            continue;
        }
        blank_run = 0;
        cleaned.push_str(line);
        cleaned.push('\n');
    }

    Ok(cleaned.trim().to_string())
}

/// 读取并校验 `~/.codex/config.toml`，返回文本（可能为空）
pub fn read_and_validate_codex_config_text() -> Result<String, AppError> {
    let s = read_codex_config_text()?;
    validate_config_toml(&s)?;
    Ok(s)
}

fn active_codex_model_provider_id(doc: &DocumentMut) -> Option<String> {
    doc.get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

pub(crate) fn is_custom_codex_model_provider_id(id: &str) -> bool {
    let id = id.trim();
    !id.is_empty()
        && !CODEX_RESERVED_MODEL_PROVIDER_IDS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(id))
}

/// Write only Codex `config.toml` for provider switching.
///
/// Codex login state lives in `auth.json`; provider routing, endpoint, model,
/// and provider-scoped bearer tokens live in `config.toml`. Provider switches
/// should not overwrite the user's ChatGPT login cache.
pub fn write_codex_live_config_atomic(config_text_opt: Option<&str>) -> Result<(), AppError> {
    let config_path = get_codex_config_path();
    let cfg_text = match config_text_opt {
        Some(config_text) => config_text.to_string(),
        None => String::new(),
    };

    if !cfg_text.trim().is_empty() {
        toml::from_str::<toml::Table>(&cfg_text).map_err(|e| AppError::toml(&config_path, e))?;
    }

    write_text_file(&config_path, &cfg_text)
}

pub fn extract_codex_auth_api_key(auth: &Value) -> Option<String> {
    auth.get("OPENAI_API_KEY")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
}

pub fn extract_codex_api_key(auth: Option<&Value>, config_text: Option<&str>) -> Option<String> {
    auth.and_then(extract_codex_auth_api_key)
        .or_else(|| config_text.and_then(extract_codex_experimental_bearer_token))
}

pub fn codex_auth_has_login_material(auth: &Value) -> bool {
    let Some(obj) = auth.as_object() else {
        return false;
    };

    obj.iter().any(|(key, value)| {
        if key == "auth_mode" {
            return false;
        }

        if key == "OPENAI_API_KEY" {
            return value
                .as_str()
                .map(str::trim)
                .is_some_and(|token| !token.is_empty());
        }

        match value {
            Value::Null => false,
            Value::String(text) => !text.trim().is_empty(),
            Value::Array(items) => !items.is_empty(),
            Value::Object(map) => !map.is_empty(),
            _ => true,
        }
    })
}

pub fn codex_auth_has_oauth_login_material(auth: &Value) -> bool {
    let Some(obj) = auth.as_object() else {
        return false;
    };

    obj.iter().any(|(key, value)| {
        if key == "auth_mode" || key == "OPENAI_API_KEY" {
            return false;
        }

        match value {
            Value::Null => false,
            Value::String(text) => !text.trim().is_empty(),
            Value::Array(items) => !items.is_empty(),
            Value::Object(map) => !map.is_empty(),
            _ => true,
        }
    })
}

pub fn should_restore_codex_provider_token_for_backfill(
    category: Option<&str>,
    template_settings: &Value,
) -> bool {
    if category == Some("official") {
        return false;
    }

    let Some(auth) = template_settings.get("auth") else {
        return true;
    };

    let has_provider_api_key = extract_codex_auth_api_key(auth).is_some();
    let has_oauth_login = codex_auth_has_oauth_login_material(auth);
    !has_oauth_login || has_provider_api_key
}

fn parse_codex_positive_u64(value: Option<&Value>) -> Option<u64> {
    match value {
        Some(Value::Number(n)) => n.as_u64().filter(|value| *value > 0),
        Some(Value::String(s)) => s.trim().parse::<u64>().ok().filter(|value| *value > 0),
        _ => None,
    }
}

fn extract_codex_top_level_u64(config_text: &str, field: &str) -> Option<u64> {
    let doc = config_text.parse::<toml::Value>().ok()?;
    doc.get(field)
        .and_then(|value| value.as_integer())
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0)
}

fn codex_catalog_model_entry(
    template: &Value,
    model: &str,
    display_name: &str,
    context_window: u64,
    priority: usize,
) -> Value {
    let mut entry = template.clone();
    let Some(entry_obj) = entry.as_object_mut() else {
        return json!({});
    };

    entry_obj.insert("slug".to_string(), json!(model));
    entry_obj.insert("display_name".to_string(), json!(display_name));
    entry_obj.insert("description".to_string(), json!(display_name));
    entry_obj.insert("context_window".to_string(), json!(context_window));
    entry_obj.insert("max_context_window".to_string(), json!(context_window));
    entry_obj.insert("priority".to_string(), json!(1000 + priority));
    entry_obj.insert("additional_speed_tiers".to_string(), json!([]));
    entry_obj.insert("service_tiers".to_string(), json!([]));
    entry_obj.insert("availability_nux".to_string(), Value::Null);
    entry_obj.insert("upgrade".to_string(), Value::Null);

    entry
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexCatalogModelSpec {
    model: String,
    display_name: String,
    context_window: u64,
}

fn codex_catalog_model_specs(settings: &Value, config_text: &str) -> Vec<CodexCatalogModelSpec> {
    let Some(models) = settings
        .get("modelCatalog")
        .and_then(|catalog| catalog.get("models"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    let default_context_window =
        extract_codex_top_level_u64(config_text, "model_context_window").unwrap_or(128_000);
    let mut seen = std::collections::HashSet::new();
    let mut specs = Vec::new();

    for model_config in models {
        let Some(model) = model_config
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
        else {
            continue;
        };

        if !seen.insert(model.to_string()) {
            continue;
        }

        let display_name = model_config
            .get("displayName")
            .or_else(|| model_config.get("display_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(model);
        let context_window = parse_codex_positive_u64(
            model_config
                .get("contextWindow")
                .or_else(|| model_config.get("context_window")),
        )
        .unwrap_or(default_context_window);

        specs.push(CodexCatalogModelSpec {
            model: model.to_string(),
            display_name: display_name.to_string(),
            context_window,
        });
    }

    specs
}

fn find_codex_model_template(catalog: &Value) -> Option<Value> {
    catalog
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| {
            models.iter().find(|model| {
                model.get("slug").and_then(Value::as_str) == Some(CODEX_MODEL_CATALOG_TEMPLATE_SLUG)
            })
        })
        .cloned()
}

fn load_codex_model_template_from_cache() -> Result<Option<Value>, AppError> {
    let path = get_codex_config_dir().join("models_cache.json");
    if !path.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?;
    let catalog: Value =
        serde_json::from_str(&text).map_err(|error| AppError::json(&path, error))?;
    Ok(find_codex_model_template(&catalog))
}

fn load_codex_model_template_from_bundled() -> Result<Option<Value>, AppError> {
    let output = match Command::new("codex")
        .args(["debug", "models", "--bundled"])
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            log::debug!("failed to run `codex debug models --bundled`: {error}");
            return Ok(None);
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::debug!("`codex debug models --bundled` failed: {stderr}");
        return Ok(None);
    }

    let catalog: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        AppError::Message(format!(
            "Failed to parse `codex debug models --bundled` output: {error}"
        ))
    })?;
    Ok(find_codex_model_template(&catalog))
}

fn load_codex_model_catalog_template() -> Result<Value, AppError> {
    if let Some(template) = load_codex_model_template_from_cache()? {
        return Ok(template);
    }
    if let Some(template) = load_codex_model_template_from_bundled()? {
        return Ok(template);
    }

    Err(AppError::Message(format!(
        "Codex model catalog template `{CODEX_MODEL_CATALOG_TEMPLATE_SLUG}` not found. Please start Codex once so models_cache.json is available, or ensure the `codex` CLI is on PATH."
    )))
}

fn codex_model_catalog_from_specs(specs: &[CodexCatalogModelSpec], template: &Value) -> Value {
    let entries: Vec<Value> = specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            codex_catalog_model_entry(
                template,
                &spec.model,
                &spec.display_name,
                spec.context_window,
                index,
            )
        })
        .collect();

    json!({ "models": entries })
}

fn codex_model_catalog_from_settings(
    settings: &Value,
    config_text: &str,
) -> Result<Option<Value>, AppError> {
    let specs = codex_catalog_model_specs(settings, config_text);
    if specs.is_empty() {
        return Ok(None);
    }

    let template = load_codex_model_catalog_template()?;
    Ok(Some(codex_model_catalog_from_specs(&specs, &template)))
}

fn set_codex_model_catalog_json_field(
    config_text: &str,
    catalog_path: Option<&Path>,
) -> Result<String, AppError> {
    let mut doc = config_text
        .parse::<DocumentMut>()
        .map_err(|error| AppError::Message(format!("Invalid Codex config.toml: {error}")))?;
    let generated_path = get_codex_model_catalog_path();

    match catalog_path {
        Some(_path) => {
            // 使用相对文件名而非绝对路径，保持与 Codex 自身引用方式一致
            doc["model_catalog_json"] = toml_edit::value(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
        }
        None => {
            let should_remove = doc
                .get("model_catalog_json")
                .and_then(|item| item.as_str())
                .map(|path| {
                    path == generated_path.to_string_lossy().as_ref()
                        || Path::new(path).file_name().and_then(|name| name.to_str())
                            == Some(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME)
                })
                .unwrap_or(false);
            if should_remove {
                doc.as_table_mut().remove("model_catalog_json");
            }
        }
    }

    Ok(doc.to_string())
}

#[derive(Clone, Debug)]
pub struct PreparedCodexConfigText {
    pub config_text: String,
    pub model_catalog: Option<Value>,
}

pub fn prepare_codex_config_text_with_model_catalog_payload(
    settings: &Value,
    config_text: &str,
) -> Result<PreparedCodexConfigText, AppError> {
    let catalog_path = get_codex_model_catalog_path();

    if let Some(catalog) = codex_model_catalog_from_settings(settings, config_text)? {
        let config_text = set_codex_model_catalog_json_field(config_text, Some(&catalog_path))?;
        Ok(PreparedCodexConfigText {
            config_text,
            model_catalog: Some(catalog),
        })
    } else {
        Ok(PreparedCodexConfigText {
            config_text: set_codex_model_catalog_json_field(config_text, None)?,
            model_catalog: None,
        })
    }
}

pub fn write_prepared_codex_model_catalog(
    prepared: &PreparedCodexConfigText,
) -> Result<(), AppError> {
    if let Some(catalog) = &prepared.model_catalog {
        write_json_file(&get_codex_model_catalog_path(), catalog)?;
    }
    Ok(())
}

pub fn prepare_codex_config_text_with_model_catalog(
    settings: &Value,
    config_text: &str,
) -> Result<String, AppError> {
    let prepared = prepare_codex_config_text_with_model_catalog_payload(settings, config_text)?;
    write_prepared_codex_model_catalog(&prepared)?;
    Ok(prepared.config_text)
}

pub fn read_codex_model_catalog_simplified_from_live() -> Result<Option<Value>, AppError> {
    let config_text = read_codex_config_text()?;
    let generated_path = get_codex_model_catalog_path();
    let Some(catalog_path) = resolve_cc_switch_catalog_path(&config_text, &generated_path) else {
        return Ok(None);
    };
    if !catalog_path.exists() {
        return Ok(None);
    }
    let Ok(catalog_text) = fs::read_to_string(&catalog_path) else {
        return Ok(None);
    };
    Ok(build_simplified_catalog_from_texts(
        &config_text,
        &catalog_text,
    ))
}

pub fn read_codex_live_settings_with_model_catalog() -> Result<Value, AppError> {
    let mut settings = read_codex_live_settings()?;
    if let Ok(Some(model_catalog)) = read_codex_model_catalog_simplified_from_live() {
        if let Some(obj) = settings.as_object_mut() {
            obj.insert("modelCatalog".to_string(), model_catalog);
        }
    }
    Ok(settings)
}

fn resolve_cc_switch_catalog_path(config_text: &str, generated_path: &Path) -> Option<PathBuf> {
    if config_text.trim().is_empty() {
        return None;
    }
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let catalog_path_str = doc
        .get("model_catalog_json")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|path| !path.is_empty())?;

    let referenced_path = Path::new(catalog_path_str);
    let is_cc_switch_owned = catalog_path_str == generated_path.to_string_lossy().as_ref()
        || referenced_path.file_name().and_then(|name| name.to_str())
            == Some(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
    if !is_cc_switch_owned {
        return None;
    }

    if referenced_path.is_absolute() {
        Some(referenced_path.to_path_buf())
    } else {
        Some(generated_path.to_path_buf())
    }
}

fn build_simplified_catalog_from_texts(config_text: &str, catalog_text: &str) -> Option<Value> {
    let catalog: Value = serde_json::from_str(catalog_text).ok()?;
    let models = catalog.get("models").and_then(Value::as_array)?;

    let default_context_window =
        extract_codex_top_level_u64(config_text, "model_context_window").unwrap_or(128_000);

    let mut entries = Vec::with_capacity(models.len());
    for entry in models {
        let Some(model) = entry
            .get("slug")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
        else {
            continue;
        };

        let mut obj = serde_json::Map::new();
        obj.insert("model".to_string(), json!(model));

        if let Some(display_name) = entry
            .get("display_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|display_name| !display_name.is_empty() && *display_name != model)
        {
            obj.insert("displayName".to_string(), json!(display_name));
        }

        if let Some(context_window) =
            entry
                .get("context_window")
                .and_then(Value::as_u64)
                .filter(|context_window| {
                    *context_window > 0 && *context_window != default_context_window
                })
        {
            obj.insert("contextWindow".to_string(), json!(context_window));
        }

        entries.push(Value::Object(obj));
    }

    if entries.is_empty() {
        return None;
    }

    Some(json!({ "models": entries }))
}

pub fn write_codex_live_with_catalog(
    settings: &Value,
    auth: &Value,
    config_text: Option<&str>,
) -> Result<(), AppError> {
    let prepared_config = config_text
        .map(|text| prepare_codex_config_text_with_model_catalog(settings, text))
        .transpose()?;

    write_codex_live_atomic(auth, prepared_config.as_deref())
}

pub fn write_codex_provider_live_with_catalog(
    settings: &Value,
    category: Option<&str>,
    auth: &Value,
    config_text: Option<&str>,
) -> Result<(), AppError> {
    let prepared_config = config_text
        .map(|text| prepare_codex_config_text_with_model_catalog(settings, text))
        .transpose()?;

    write_codex_live_for_provider(category, auth, prepared_config.as_deref())
}

/// Extract a provider-scoped `experimental_bearer_token` from Codex `config.toml`.
///
/// Third-party providers may store the API key inside
/// `[model_providers.<id>].experimental_bearer_token` while keeping the
/// user's ChatGPT login cache intact in `auth.json`. Falls back to the
/// top-level `experimental_bearer_token` when no active model provider is set.
pub fn extract_codex_experimental_bearer_token(config_text: &str) -> Option<String> {
    if !config_text.contains("experimental_bearer_token") {
        return None;
    }
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let provider_id = active_codex_model_provider_id(&doc);

    let top_level_token = || {
        doc.get("experimental_bearer_token")
            .and_then(|item| item.as_str())
    };
    let token = match provider_id.as_deref() {
        Some(id) if is_custom_codex_model_provider_id(id) => doc
            .get("model_providers")
            .and_then(|item| item.as_table())
            .and_then(|table| table.get(id))
            .and_then(|item| item.as_table())
            .and_then(|table| table.get("experimental_bearer_token"))
            .and_then(|item| item.as_str())
            .or_else(top_level_token),
        Some(_) => top_level_token(),
        None => top_level_token(),
    };

    token
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

fn set_codex_experimental_bearer_token(config_text: &str, token: &str) -> Result<String, AppError> {
    if config_text.trim().is_empty() {
        return Err(AppError::localized(
            "provider.codex.config.missing",
            "Codex 第三方供应商缺少 config.toml 配置，无法写入 bearer token",
            "Codex third-party provider is missing config.toml, cannot write bearer token",
        ));
    }

    let mut doc = config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?;

    let Some(provider_id) = active_codex_model_provider_id(&doc) else {
        doc["experimental_bearer_token"] = toml_edit::value(token);
        return Ok(doc.to_string());
    };

    if !is_custom_codex_model_provider_id(&provider_id) {
        // Reserved Codex provider IDs are owned by the CLI. Keep third-party
        // bearer tokens at the top level so we do not shadow built-in tables.
        doc["experimental_bearer_token"] = toml_edit::value(token);
        return Ok(doc.to_string());
    }

    if let Some(model_providers) = doc
        .get_mut("model_providers")
        .and_then(|item| item.as_table_mut())
    {
        if let Some(provider_table) = model_providers
            .get_mut(provider_id.as_str())
            .and_then(|item| item.as_table_mut())
        {
            provider_table["experimental_bearer_token"] = toml_edit::value(token);
            return Ok(doc.to_string());
        }
    }

    doc["experimental_bearer_token"] = toml_edit::value(token);
    Ok(doc.to_string())
}

pub fn remove_codex_experimental_bearer_token_if(
    config_text: &str,
    predicate: impl Fn(&str) -> bool,
) -> Result<String, AppError> {
    if config_text.trim().is_empty() || !config_text.contains("experimental_bearer_token") {
        return Ok(config_text.to_string());
    }

    let mut doc = config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?;

    if let Some(provider_id) = active_codex_model_provider_id(&doc) {
        if let Some(provider_table) = doc
            .get_mut("model_providers")
            .and_then(|item| item.as_table_mut())
            .and_then(|table| table.get_mut(provider_id.as_str()))
            .and_then(|item| item.as_table_mut())
        {
            let should_remove = provider_table
                .get("experimental_bearer_token")
                .and_then(|item| item.as_str())
                .map(str::trim)
                .is_some_and(&predicate);
            if should_remove {
                provider_table.remove("experimental_bearer_token");
            }
        }
    }

    let should_remove_top_level = doc
        .get("experimental_bearer_token")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .is_some_and(&predicate);
    if should_remove_top_level {
        doc.as_table_mut().remove("experimental_bearer_token");
    }
    Ok(doc.to_string())
}

fn remove_codex_experimental_bearer_token(config_text: &str) -> Result<String, AppError> {
    remove_codex_experimental_bearer_token_if(config_text, |_| true)
}

/// Read the current Codex live settings as a `{ auth, config }` object.
///
/// Missing `auth.json` collapses to `{}` so a config-only third-party install
/// is still importable; both files empty is treated as "no live install".
pub fn read_codex_live_settings() -> Result<Value, AppError> {
    let auth_path = get_codex_auth_path();
    let auth_present = auth_path.exists();
    let auth: Value = if auth_present {
        read_json_file(&auth_path)?
    } else {
        json!({})
    };
    let cfg_text = read_and_validate_codex_config_text()?;
    if !auth_present && cfg_text.trim().is_empty() {
        return Err(AppError::localized(
            "codex.live.missing",
            "Codex 配置文件不存在",
            "Codex configuration is missing",
        ));
    }
    Ok(json!({ "auth": auth, "config": cfg_text }))
}

/// `[model_providers.custom]` entry that makes an official (ChatGPT OAuth)
/// provider behave like Codex's built-in `openai` entry while running under
/// the shared custom id.
fn codex_unified_official_provider_table() -> toml_edit::Table {
    let mut table = toml_edit::Table::new();
    table["name"] = toml_edit::value("OpenAI");
    table["requires_openai_auth"] = toml_edit::value(true);
    table["supports_websockets"] = toml_edit::value(true);
    table["wire_api"] = toml_edit::value("responses");
    table
}

fn table_matches_codex_unified_official_provider(table: &toml_edit::Table) -> bool {
    table.len() == 4
        && table.get("name").and_then(|item| item.as_str()) == Some("OpenAI")
        && table
            .get("requires_openai_auth")
            .and_then(|item| item.as_bool())
            == Some(true)
        && table
            .get("supports_websockets")
            .and_then(|item| item.as_bool())
            == Some(true)
        && table.get("wire_api").and_then(|item| item.as_str()) == Some("responses")
}

/// 统一 Codex 会话历史：把官方供应商的 live 配置改写为以共享的
/// `custom` model_provider 标识运行（认证仍走 `auth.json` 的 ChatGPT 登录）。
pub fn inject_codex_unified_session_bucket(config_text: &str) -> Result<String, AppError> {
    let mut doc = config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?;

    if doc.get("model_provider").is_some() {
        return Ok(config_text.to_string());
    }

    let existing_custom_conflicts = doc
        .get("model_providers")
        .and_then(|item| item.as_table())
        .and_then(|providers| providers.get(CC_SWITCH_CODEX_MODEL_PROVIDER_ID))
        .and_then(|item| item.as_table())
        .is_some_and(|table| !table_matches_codex_unified_official_provider(table));
    if existing_custom_conflicts {
        log::warn!(
            "官方 Codex 配置已存在自定义 [model_providers.custom]，跳过统一会话路由注入以避免激活未知路由"
        );
        return Ok(config_text.to_string());
    }

    doc["model_provider"] = toml_edit::value(CC_SWITCH_CODEX_MODEL_PROVIDER_ID);

    if doc.get("model_providers").is_none() {
        let mut parent = toml_edit::Table::new();
        parent.set_implicit(true);
        doc["model_providers"] = toml_edit::Item::Table(parent);
    }
    if let Some(providers) = doc["model_providers"].as_table_mut() {
        if !providers.contains_key(CC_SWITCH_CODEX_MODEL_PROVIDER_ID) {
            providers.insert(
                CC_SWITCH_CODEX_MODEL_PROVIDER_ID,
                toml_edit::Item::Table(codex_unified_official_provider_table()),
            );
        }
    }

    Ok(doc.to_string())
}

/// `inject_codex_unified_session_bucket` 的反向操作：仅当形态与注入产物完全一致时剥离。
pub fn strip_codex_unified_session_bucket(config_text: &str) -> Result<String, AppError> {
    if !config_text.contains("model_provider") {
        return Ok(config_text.to_string());
    }

    let mut doc = config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?;

    if doc.get("model_provider").and_then(|item| item.as_str())
        != Some(CC_SWITCH_CODEX_MODEL_PROVIDER_ID)
    {
        return Ok(config_text.to_string());
    }

    let matches_injected = doc
        .get("model_providers")
        .and_then(|item| item.as_table())
        .and_then(|providers| providers.get(CC_SWITCH_CODEX_MODEL_PROVIDER_ID))
        .and_then(|item| item.as_table())
        .is_some_and(table_matches_codex_unified_official_provider);
    if !matches_injected {
        return Ok(config_text.to_string());
    }

    doc.as_table_mut().remove("model_provider");
    let providers_empty = doc["model_providers"]
        .as_table_mut()
        .map(|providers| {
            providers.remove(CC_SWITCH_CODEX_MODEL_PROVIDER_ID);
            providers.is_empty()
        })
        .unwrap_or(false);
    if providers_empty {
        doc.as_table_mut().remove("model_providers");
    }

    Ok(doc.to_string())
}

/// 统一会话开关开启时，把官方供应商 `{ auth, config }` 设置对象中的
/// config 文本注入共享 custom 路由；开关关闭或非官方供应商时不做改动。
pub fn apply_codex_unified_session_bucket_to_settings(
    category: Option<&str>,
    settings: &mut Value,
) -> Result<(), AppError> {
    if category != Some("official") || !crate::settings::unify_codex_session_history() {
        return Ok(());
    }

    let config_text = settings
        .get("config")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let injected = inject_codex_unified_session_bucket(&config_text)?;
    if injected != config_text {
        if let Some(obj) = settings.as_object_mut() {
            obj.insert("config".to_string(), Value::String(injected));
        }
    }
    Ok(())
}

/// Backfill helper: strip the unified-session injection from a live
/// `{ auth, config }` settings object before it is stored back to the DB.
pub fn strip_codex_unified_session_bucket_from_settings(
    settings: &mut Value,
) -> Result<(), AppError> {
    let Some(config_text) = settings
        .get("config")
        .and_then(|value| value.as_str())
        .map(str::to_string)
    else {
        return Ok(());
    };

    let stripped = strip_codex_unified_session_bucket(&config_text)?;
    if stripped != config_text {
        if let Some(obj) = settings.as_object_mut() {
            obj.insert("config".to_string(), Value::String(stripped));
        }
    }
    Ok(())
}

/// Route a Codex live write between full auth+config or config-only.
///
/// Official providers with usable login material own `auth.json`. Third-party
/// providers only touch `config.toml` when the compatibility setting is enabled
/// so the user's ChatGPT login cache survives provider switches.
pub fn write_codex_provider_live_config_only_with_catalog(
    settings: &Value,
    auth: &Value,
    config_text: Option<&str>,
) -> Result<(), AppError> {
    let prepared_config = config_text
        .map(|text| prepare_codex_config_text_with_model_catalog(settings, text))
        .transpose()?;
    let live_config =
        prepare_codex_provider_live_config(auth, prepared_config.as_deref().unwrap_or(""))?;
    write_codex_live_config_atomic(Some(&live_config))
}

pub fn write_codex_live_for_provider(
    category: Option<&str>,
    auth: &Value,
    config_text: Option<&str>,
) -> Result<(), AppError> {
    let unified_official_config =
        if category == Some("official") && crate::settings::unify_codex_session_history() {
            Some(inject_codex_unified_session_bucket(
                config_text.unwrap_or(""),
            )?)
        } else {
            None
        };
    let config_text = unified_official_config.as_deref().or(config_text);

    let should_write_auth = (category == Some("official") && codex_auth_has_login_material(auth))
        || (category != Some("official")
            && !crate::settings::preserve_codex_official_auth_on_switch());

    if should_write_auth {
        write_codex_live_atomic(auth, config_text)
    } else {
        let live_config = prepare_codex_provider_live_config(auth, config_text.unwrap_or(""))?;
        write_codex_live_config_atomic(Some(&live_config))
    }
}

/// Build the live Codex config for provider switching.
///
/// The stored provider keeps its API key in `auth.OPENAI_API_KEY`. Live Codex
/// requests can use a provider-scoped `experimental_bearer_token`, so switching
/// providers only needs to update `config.toml`; `auth.json` stays as the user's
/// long-lived ChatGPT login cache.
pub fn prepare_codex_provider_live_config(
    auth: &Value,
    config_text: &str,
) -> Result<String, AppError> {
    let token = extract_codex_auth_api_key(auth)
        .or_else(|| extract_codex_experimental_bearer_token(config_text));

    Ok(match token {
        Some(token) => set_codex_experimental_bearer_token(config_text, &token)?,
        None => config_text.to_string(),
    })
}

/// During DB backfill, lift a live `experimental_bearer_token` back into
/// `auth.OPENAI_API_KEY` so the stored provider keeps its canonical shape
/// and generated live tokens don't leak into stored provider TOML.
///
/// Only intervenes when the live config actually carries a bearer token;
/// otherwise the function is a no-op so the caller's normal backfill path
/// remains authoritative.
pub fn restore_codex_provider_token_for_backfill(
    settings: &mut Value,
    template_settings: &Value,
) -> Result<(), AppError> {
    let Some(config_text) = settings
        .get("config")
        .and_then(|value| value.as_str())
        .map(str::to_string)
    else {
        return Ok(());
    };

    let Some(token) = extract_codex_experimental_bearer_token(&config_text) else {
        return Ok(());
    };

    let cleaned_config = remove_codex_experimental_bearer_token(&config_text)?;

    if let Some(obj) = settings.as_object_mut() {
        obj.insert("config".to_string(), Value::String(cleaned_config));

        let mut auth = template_settings
            .get("auth")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        if let Some(auth_obj) = auth.as_object_mut() {
            auth_obj.insert("OPENAI_API_KEY".to_string(), Value::String(token));
        }
        obj.insert("auth".to_string(), auth);
    }

    Ok(())
}

pub fn restore_codex_settings_for_backfill(
    settings: &mut Value,
    template_settings: &Value,
    restore_provider_token: bool,
) -> Result<(), AppError> {
    if restore_provider_token {
        restore_codex_provider_token_for_backfill(settings, template_settings)?;
    }
    if let (Some(settings_obj), Some(model_catalog)) = (
        settings.as_object_mut(),
        template_settings.get("modelCatalog").cloned(),
    ) {
        settings_obj.insert("modelCatalog".to_string(), model_catalog);
    }
    Ok(())
}

/// Generate a clean TOML key from a raw string for use as `model_provider` and `[model_providers.<key>]`.
///
/// Lowercases ASCII alphanumerics, replaces everything else with `_`, trims leading/trailing `_`.
/// Falls back to `"custom"` if the result is empty.
pub fn clean_codex_provider_key(raw: &str) -> String {
    let mut key: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();

    while key.starts_with('_') {
        key.remove(0);
    }
    while key.ends_with('_') {
        key.pop();
    }

    if key.is_empty() {
        "custom".to_string()
    } else {
        key
    }
}

pub fn build_codex_provider_config_toml(
    provider_key: &str,
    base_url: &str,
    model: &str,
    wire_api: &str,
) -> String {
    let provider_key = escape_toml_string(provider_key);
    let model = escape_toml_string(model);
    let base_url = escape_toml_string(base_url);
    let wire_api = escape_toml_string(wire_api);

    [
        format!("model_provider = \"{}\"", provider_key),
        format!("model = \"{}\"", model),
        "model_reasoning_effort = \"high\"".to_string(),
        "disable_response_storage = true".to_string(),
        String::new(),
        format!("[model_providers.{}]", provider_key),
        format!("name = \"{}\"", provider_key),
        format!("base_url = \"{}\"", base_url),
        format!("wire_api = \"{}\"", wire_api),
        "requires_openai_auth = true".to_string(),
        String::new(),
    ]
    .join("\n")
}

pub fn update_codex_config_snippet(
    original: &str,
    base_url: &str,
    model: &str,
    wire_api: &str,
    requires_openai_auth: bool,
    env_key: &str,
) -> String {
    let mut doc = match original.trim().parse::<toml_edit::DocumentMut>() {
        Ok(doc) => doc,
        Err(_) => return original.to_string(),
    };

    if let Some(model) = non_empty(model) {
        doc["model"] = toml_edit::value(model);
    } else {
        doc.remove("model");
    }

    let provider_key = doc
        .get("model_provider")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());

    if let Some(key) = provider_key {
        if doc.get("model_providers").is_none() {
            doc["model_providers"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let providers = doc["model_providers"]
            .as_table_like_mut()
            .expect("model_providers should be a table");
        if providers.get(&key).is_none() {
            providers.insert(&key, toml_edit::Item::Table(toml_edit::Table::new()));
        }

        if let Some(section) = providers
            .get_mut(&key)
            .and_then(|value| value.as_table_like_mut())
        {
            if let Some(base_url) = non_empty(base_url) {
                section.insert("base_url", toml_edit::value(base_url));
            } else {
                section.remove("base_url");
            }

            section.insert("wire_api", toml_edit::value(wire_api));
            section.insert(
                "requires_openai_auth",
                toml_edit::value(requires_openai_auth),
            );

            if requires_openai_auth {
                section.remove("env_key");
            } else {
                let env_key = non_empty(env_key).unwrap_or("OPENAI_API_KEY");
                section.insert("env_key", toml_edit::value(env_key));
            }
        }
    }

    let result = doc.to_string();
    let trimmed = result.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        trimmed.to_string()
    }
}

/// Normalize persisted Codex provider config to upstream semantics.
///
/// Codex providers should keep `wire_api = "responses"` in the Codex config
/// TOML. Local Responses <-> Chat routing is controlled by provider
/// `meta.apiFormat`, not by persisting `wire_api = "chat"`.
pub fn normalize_codex_config_wire_api_to_responses(config_text: &str) -> String {
    let mut doc = match config_text.trim().parse::<toml_edit::DocumentMut>() {
        Ok(doc) => doc,
        Err(_) => return config_text.to_string(),
    };

    let mut updated = false;
    if let Some(provider_key) = active_codex_model_provider_id(&doc) {
        if let Some(section) = doc
            .get_mut("model_providers")
            .and_then(|providers| providers.as_table_like_mut())
            .and_then(|providers| providers.get_mut(&provider_key))
            .and_then(|provider| provider.as_table_like_mut())
        {
            section.insert("wire_api", toml_edit::value("responses"));
            updated = true;
        }
    }

    if doc.get("wire_api").is_some() {
        doc["wire_api"] = toml_edit::value("responses");
        updated = true;
    }

    if updated {
        doc.to_string().trim().to_string()
    } else {
        config_text.to_string()
    }
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn escape_toml_string(value: &str) -> String {
    value.replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{lock_test_home_and_settings, set_test_home_override};
    use std::env;
    use std::ffi::OsString;
    use tempfile::TempDir;

    struct CodexHomeEnvGuard {
        original: Option<OsString>,
    }

    impl CodexHomeEnvGuard {
        fn new(value: Option<&str>) -> Self {
            let original = env::var_os("CODEX_HOME");
            match value {
                Some(value) => unsafe { env::set_var("CODEX_HOME", value) },
                None => unsafe { env::remove_var("CODEX_HOME") },
            }
            Self { original }
        }
    }

    impl Drop for CodexHomeEnvGuard {
        fn drop(&mut self) {
            match self.original.as_ref() {
                Some(value) => unsafe { env::set_var("CODEX_HOME", value) },
                None => unsafe { env::remove_var("CODEX_HOME") },
            }
        }
    }

    struct SettingsGuard {
        original: crate::settings::AppSettings,
    }

    impl SettingsGuard {
        fn with_codex_config_dir(dir: Option<&str>) -> Self {
            let original = crate::settings::get_settings();
            if let Some(home) = crate::config::home_dir() {
                fs::create_dir_all(home).unwrap();
            }
            let mut settings = original.clone();
            settings.codex_config_dir = dir.map(str::to_string);
            crate::settings::update_settings(settings).unwrap();
            Self { original }
        }
    }

    impl Drop for SettingsGuard {
        fn drop(&mut self) {
            let _ = crate::settings::update_settings(self.original.clone());
        }
    }

    #[test]
    fn normalize_codex_config_wire_api_updates_active_provider_and_top_level() {
        let normalized = normalize_codex_config_wire_api_to_responses(
            r#"model_provider = "vendor"
wire_api = "chat"

[model_providers.vendor]
base_url = "https://vendor.example/v1"
wire_api = "chat"

[model_providers.other]
base_url = "https://other.example/v1"
wire_api = "chat"
"#,
        );

        let parsed = normalized
            .parse::<toml_edit::DocumentMut>()
            .expect("normalized config should stay valid TOML");
        assert_eq!(parsed["wire_api"].as_str(), Some("responses"));
        assert_eq!(
            parsed["model_providers"]["vendor"]["wire_api"].as_str(),
            Some("responses")
        );
        assert_eq!(
            parsed["model_providers"]["other"]["wire_api"].as_str(),
            Some("chat"),
            "inactive providers should be left alone"
        );
    }

    #[test]
    fn normalize_codex_config_wire_api_updates_top_level_only_config() {
        let normalized = normalize_codex_config_wire_api_to_responses(
            r#"wire_api = "chat"
model = "gpt-5.4"
"#,
        );

        let parsed = normalized
            .parse::<toml_edit::DocumentMut>()
            .expect("normalized config should stay valid TOML");
        assert_eq!(parsed["wire_api"].as_str(), Some("responses"));
        assert_eq!(parsed["model"].as_str(), Some("gpt-5.4"));
    }

    #[test]
    fn normalize_codex_config_wire_api_preserves_invalid_toml() {
        let config = "model_provider = ";

        assert_eq!(normalize_codex_config_wire_api_to_responses(config), config);
    }

    #[test]
    fn get_codex_config_dir_respects_codex_home_env_var_when_directory_exists() {
        let _guard = lock_test_home_and_settings();
        set_test_home_override(Some(Path::new("/tmp/codex-home-env-home")));
        let _settings = SettingsGuard::with_codex_config_dir(None);
        let codex_home =
            std::env::temp_dir().join(format!("cc-switch-codex-home-env-{}", std::process::id()));
        fs::create_dir_all(&codex_home).unwrap();
        let _env = CodexHomeEnvGuard::new(codex_home.to_str());

        assert_eq!(get_codex_config_dir(), codex_home);

        set_test_home_override(None);
    }

    #[test]
    fn get_codex_config_dir_falls_back_to_home_dot_codex_when_codex_home_unset() {
        let _guard = lock_test_home_and_settings();
        set_test_home_override(Some(Path::new("/tmp/codex-default-home")));
        let _settings = SettingsGuard::with_codex_config_dir(None);
        let _env = CodexHomeEnvGuard::new(None);

        assert_eq!(
            get_codex_config_dir(),
            PathBuf::from("/tmp/codex-default-home").join(".codex")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_codex_config_dir_blank_codex_home_uses_settings_override() {
        let _guard = lock_test_home_and_settings();
        set_test_home_override(Some(Path::new("/tmp/codex-blank-env-home")));
        let _settings = SettingsGuard::with_codex_config_dir(Some("/tmp/codex-settings-dir"));
        let _env = CodexHomeEnvGuard::new(Some("   "));

        assert_eq!(
            get_codex_config_dir(),
            PathBuf::from("/tmp/codex-settings-dir")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_codex_config_dir_nonexistent_codex_home_uses_settings_override() {
        let _guard = lock_test_home_and_settings();
        set_test_home_override(Some(Path::new("/tmp/codex-nonexistent-env-home")));
        let _settings = SettingsGuard::with_codex_config_dir(Some("/tmp/codex-settings-dir"));
        let missing = std::env::temp_dir().join(format!(
            "cc-switch-codex-missing-env-{}",
            std::process::id()
        ));
        let _env = CodexHomeEnvGuard::new(missing.to_str());

        assert_eq!(
            get_codex_config_dir(),
            PathBuf::from("/tmp/codex-settings-dir")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_codex_config_dir_file_codex_home_falls_back_to_home_dot_codex() {
        let _guard = lock_test_home_and_settings();
        set_test_home_override(Some(Path::new("/tmp/codex-file-env-home")));
        let _settings = SettingsGuard::with_codex_config_dir(None);
        let codex_home_file = std::env::temp_dir().join(format!(
            "cc-switch-codex-home-env-file-{}",
            std::process::id()
        ));
        fs::write(&codex_home_file, "not a directory").unwrap();
        let _env = CodexHomeEnvGuard::new(codex_home_file.to_str());

        assert_eq!(
            get_codex_config_dir(),
            PathBuf::from("/tmp/codex-file-env-home").join(".codex")
        );

        let _ = fs::remove_file(codex_home_file);
        set_test_home_override(None);
    }

    #[test]
    fn get_codex_config_dir_settings_override_takes_precedence_over_codex_home() {
        let _guard = lock_test_home_and_settings();
        set_test_home_override(Some(Path::new("/tmp/codex-precedence-home")));
        let _settings = SettingsGuard::with_codex_config_dir(Some("/tmp/codex-settings-dir"));
        let codex_home = std::env::temp_dir().join(format!(
            "cc-switch-codex-precedence-env-{}",
            std::process::id()
        ));
        fs::create_dir_all(&codex_home).unwrap();
        let _env = CodexHomeEnvGuard::new(codex_home.to_str());

        assert_eq!(
            get_codex_config_dir(),
            PathBuf::from("/tmp/codex-settings-dir")
        );

        let _ = fs::remove_dir_all(codex_home);
        set_test_home_override(None);
    }

    #[test]
    fn unified_session_bucket_injects_for_empty_official_config() {
        let injected = inject_codex_unified_session_bucket("").expect("inject");
        let doc: toml::Table = toml::from_str(&injected).expect("parse injected config");

        assert_eq!(
            doc.get("model_provider").and_then(|value| value.as_str()),
            Some(CC_SWITCH_CODEX_MODEL_PROVIDER_ID)
        );
        let custom = doc["model_providers"][CC_SWITCH_CODEX_MODEL_PROVIDER_ID]
            .as_table()
            .expect("custom provider table");
        assert_eq!(
            custom.get("name").and_then(|value| value.as_str()),
            Some("OpenAI")
        );
        assert_eq!(
            custom
                .get("requires_openai_auth")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            custom
                .get("supports_websockets")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            custom.get("wire_api").and_then(|value| value.as_str()),
            Some("responses")
        );
    }

    #[test]
    fn unified_session_bucket_preserves_other_keys_and_explicit_routing() {
        let with_catalog = "model_catalog_json = \"cc-switch-model-catalog.json\"\n";
        let injected = inject_codex_unified_session_bucket(with_catalog).expect("inject");
        assert!(injected.contains("model_catalog_json"));
        assert!(injected.contains("model_provider = \"custom\""));

        let explicit = "model_provider = \"openai_https\"\n";
        let unchanged = inject_codex_unified_session_bucket(explicit).expect("inject");
        assert_eq!(unchanged, explicit);
    }

    #[test]
    fn unified_session_bucket_skips_conflicting_custom_table() {
        let stale = r#"[model_providers.custom]
name = "Relay"
base_url = "https://relay.example/v1"
"#;
        let unchanged = inject_codex_unified_session_bucket(stale).expect("inject");
        assert_eq!(unchanged, stale);

        let injected_once = inject_codex_unified_session_bucket("").expect("inject");
        let reinjected = inject_codex_unified_session_bucket(&injected_once).expect("re-inject");
        assert_eq!(reinjected, injected_once);
    }

    #[test]
    fn unified_session_bucket_strip_round_trips_injection() {
        let injected = inject_codex_unified_session_bucket("").expect("inject");
        let stripped = strip_codex_unified_session_bucket(&injected).expect("strip");
        assert_eq!(stripped.trim(), "");

        let with_catalog = "model_catalog_json = \"cc-switch-model-catalog.json\"\n";
        let injected = inject_codex_unified_session_bucket(with_catalog).expect("inject");
        let stripped = strip_codex_unified_session_bucket(&injected).expect("strip");
        assert_eq!(stripped, with_catalog);
    }

    #[test]
    fn unified_session_bucket_strip_keeps_third_party_custom_entry() {
        let third_party = r#"model_provider = "custom"

[model_providers.custom]
name = "Relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#;
        let untouched = strip_codex_unified_session_bucket(third_party).expect("strip");
        assert_eq!(untouched, third_party);
    }

    #[test]
    fn unified_session_bucket_strip_from_settings_only_touches_config() {
        let injected = inject_codex_unified_session_bucket("").expect("inject");
        let mut settings = json!({
            "auth": { "tokens": { "access_token": "secret" } },
            "config": injected,
        });
        strip_codex_unified_session_bucket_from_settings(&mut settings).expect("strip settings");
        assert_eq!(
            settings
                .get("config")
                .and_then(|value| value.as_str())
                .map(str::trim),
            Some("")
        );
        assert!(settings.pointer("/auth/tokens/access_token").is_some());
    }

    #[test]
    fn prepare_provider_live_config_writes_provider_scoped_bearer_token() {
        let input = r#"model_provider = "vendor_alpha"
model = "gpt-5.4"

[model_providers.vendor_alpha]
name = "Vendor Alpha"
base_url = "https://alpha.example/v1"
wire_api = "responses"
"#;

        let result =
            prepare_codex_provider_live_config(&json!({"OPENAI_API_KEY": "sk-test"}), input)
                .expect("prepare live config");
        let parsed: toml::Value = toml::from_str(&result).expect("parse prepared config");

        assert_eq!(
            parsed
                .get("model_providers")
                .and_then(|v| v.get("vendor_alpha"))
                .and_then(|v| v.get("experimental_bearer_token"))
                .and_then(|v| v.as_str()),
            Some("sk-test")
        );
        assert_eq!(
            extract_codex_experimental_bearer_token(&result).as_deref(),
            Some("sk-test")
        );
    }

    #[test]
    fn remove_codex_experimental_bearer_token_if_only_removes_matching_values() {
        let input = r#"experimental_bearer_token = "sk-real"
model_provider = "vendor_alpha"
model = "gpt-5.4"

[model_providers.vendor_alpha]
name = "Vendor Alpha"
base_url = "https://alpha.example/v1"
wire_api = "responses"
experimental_bearer_token = "PROXY_MANAGED"
"#;

        let result =
            remove_codex_experimental_bearer_token_if(input, |token| token == "PROXY_MANAGED")
                .expect("remove matching proxy placeholder");
        let parsed: toml::Value = toml::from_str(&result).expect("parse cleaned config");

        assert_eq!(
            parsed
                .get("experimental_bearer_token")
                .and_then(|value| value.as_str()),
            Some("sk-real"),
            "real top-level token must not be removed by placeholder cleanup"
        );
        assert!(
            parsed
                .get("model_providers")
                .and_then(|value| value.get("vendor_alpha"))
                .and_then(|value| value.get("experimental_bearer_token"))
                .is_none(),
            "provider-scoped proxy placeholder should be removed"
        );
    }

    fn test_codex_model_template() -> Value {
        json!({
            "slug": "gpt-5.5",
            "display_name": "GPT-5.5",
            "description": "Frontier model",
            "base_instructions": "gpt-5.5 base instructions",
            "model_messages": {
                "instructions_template": "gpt-5.5 instructions template",
                "instructions_variables": {
                    "personality_default": "",
                    "personality_friendly": "",
                    "personality_pragmatic": ""
                }
            },
            "additional_speed_tiers": ["fast"],
            "service_tiers": [
                {
                    "id": "priority",
                    "name": "Fast",
                    "description": "1.5x speed, increased usage"
                }
            ],
            "availability_nux": {
                "message": "GPT-5.5 is now available."
            },
            "upgrade": {
                "target": "gpt-5.5"
            },
            "context_window": 272000,
            "max_context_window": 272000
        })
    }

    #[test]
    fn codex_model_catalog_uses_provider_models_and_context() {
        let template = test_codex_model_template();
        let settings = json!({
            "modelCatalog": {
                "models": [
                    {
                        "model": "deepseek-v4-flash",
                        "displayName": "DeepSeek V4 Flash",
                        "contextWindow": "64000"
                    },
                    {
                        "model": "kimi-k2",
                        "display_name": "Kimi K2"
                    }
                ]
            }
        });
        let specs = codex_catalog_model_specs(&settings, r#"model_context_window = 128000"#);
        let catalog = codex_model_catalog_from_specs(&specs, &template);
        let models = catalog
            .get("models")
            .and_then(Value::as_array)
            .expect("models should be an array");

        assert_eq!(models.len(), 2);
        assert_eq!(
            models[0].get("slug").and_then(Value::as_str),
            Some("deepseek-v4-flash")
        );
        assert_eq!(
            models[0].get("context_window").and_then(Value::as_u64),
            Some(64_000)
        );
        assert_eq!(
            models[1].get("context_window").and_then(Value::as_u64),
            Some(128_000)
        );
        assert!(
            models[0].get("model_messages").is_some(),
            "Codex requires model_messages in custom catalogs"
        );
        assert_eq!(
            models[0].get("base_instructions").and_then(Value::as_str),
            Some("gpt-5.5 base instructions")
        );
        assert_eq!(
            models[0].get("model_messages"),
            template.get("model_messages"),
            "custom catalog entries should keep the gpt-5.5 agent template"
        );
        assert_eq!(
            models[0].get("additional_speed_tiers"),
            Some(&json!([])),
            "generated third-party entries should not inherit OpenAI speed tiers"
        );
        assert!(
            models[0]
                .get("availability_nux")
                .is_some_and(Value::is_null),
            "generated third-party entries should not inherit GPT-5.5 launch messaging"
        );
    }

    #[test]
    fn model_catalog_json_field_operates_on_top_level() {
        let input = r#"model_provider = "any"

[model_providers.any]
name = "any"
"#;
        let catalog_path = Path::new("/tmp/cc-switch-model-catalog.json");

        let result = set_codex_model_catalog_json_field(input, Some(catalog_path)).unwrap();
        let parsed: toml::Value = toml::from_str(&result).unwrap();
        assert_eq!(
            parsed
                .get("model_catalog_json")
                .and_then(toml::Value::as_str),
            Some("cc-switch-model-catalog.json")
        );
        assert!(
            parsed
                .get("model_providers")
                .and_then(|value| value.get("any"))
                .and_then(|value| value.get("model_catalog_json"))
                .is_none(),
            "model_catalog_json should stay top-level"
        );
    }

    #[test]
    fn resolve_catalog_path_returns_none_when_config_missing_field() {
        let generated = PathBuf::from("/tmp/.codex/cc-switch-model-catalog.json");
        assert!(resolve_cc_switch_catalog_path("", &generated).is_none());
        assert!(
            resolve_cc_switch_catalog_path("model = \"gpt-5\"", &generated).is_none(),
            "no model_catalog_json field should yield None"
        );
    }

    #[test]
    fn resolve_catalog_path_accepts_cc_switch_owned_file() {
        let generated = PathBuf::from("/tmp/.codex/cc-switch-model-catalog.json");
        let config = r#"model_catalog_json = "/tmp/.codex/cc-switch-model-catalog.json"
"#;
        let resolved = resolve_cc_switch_catalog_path(config, &generated).expect("path resolves");
        assert_eq!(resolved, generated);
    }

    #[test]
    fn resolve_catalog_path_rejects_user_owned_external_file() {
        let generated = PathBuf::from("/tmp/.codex/cc-switch-model-catalog.json");
        let config = r#"model_catalog_json = "/Users/me/.codex/my-handwritten-catalog.json"
"#;
        assert!(
            resolve_cc_switch_catalog_path(config, &generated).is_none(),
            "external catalog files should be left alone"
        );
    }

    #[test]
    fn build_simplified_catalog_round_trips_user_input() {
        let catalog = r#"{
            "models": [
                { "slug": "deepseek-v4-pro", "display_name": "deepseek-v4-pro", "context_window": 1000000 },
                { "slug": "deepseek-v4-flash", "display_name": "DeepSeek Flash", "context_window": 1000000 }
            ]
        }"#;
        let result = build_simplified_catalog_from_texts("", catalog).expect("entries found");
        let models = result
            .get("models")
            .and_then(Value::as_array)
            .expect("models array");
        assert_eq!(models.len(), 2);

        assert_eq!(
            models[0].get("model").and_then(Value::as_str),
            Some("deepseek-v4-pro")
        );
        assert!(models[0].get("displayName").is_none());
        assert_eq!(
            models[0].get("contextWindow").and_then(Value::as_u64),
            Some(1_000_000)
        );
        assert_eq!(
            models[1].get("displayName").and_then(Value::as_str),
            Some("DeepSeek Flash")
        );
    }

    #[test]
    fn build_simplified_catalog_squashes_default_context_window() {
        let catalog = r#"{
            "models": [{ "slug": "kimi", "display_name": "kimi", "context_window": 128000 }]
        }"#;
        let result = build_simplified_catalog_from_texts("", catalog).expect("entry");
        let entry = &result.get("models").unwrap().as_array().unwrap()[0];
        assert!(
            entry.get("contextWindow").is_none(),
            "default 128_000 should be squashed so the form shows blank"
        );
    }

    #[test]
    fn build_simplified_catalog_respects_explicit_model_context_window() {
        let config = r#"model_context_window = 200000
"#;
        let catalog = r#"{
            "models": [
                { "slug": "a", "display_name": "a", "context_window": 200000 },
                { "slug": "b", "display_name": "b", "context_window": 500000 }
            ]
        }"#;
        let result = build_simplified_catalog_from_texts(config, catalog).expect("entries");
        let models = result.get("models").unwrap().as_array().unwrap();
        assert!(models[0].get("contextWindow").is_none());
        assert_eq!(
            models[1].get("contextWindow").and_then(Value::as_u64),
            Some(500_000)
        );
    }

    #[test]
    fn build_simplified_catalog_returns_none_when_unparseable() {
        assert!(build_simplified_catalog_from_texts("", "not json").is_none());
        assert!(build_simplified_catalog_from_texts("", "{}").is_none());
        assert!(
            build_simplified_catalog_from_texts("", r#"{"models": []}"#).is_none(),
            "empty models array should yield None so the field is not inserted at all"
        );
        assert!(
            build_simplified_catalog_from_texts(
                "",
                r#"{"models": [{"display_name": "no slug"}]}"#,
            )
            .is_none(),
            "entries lacking slug are skipped; a fully-skipped catalog yields None"
        );
    }

    #[test]
    fn provider_live_write_projects_model_catalog_to_codex_files() {
        let _guard = lock_test_home_and_settings();
        let temp_home = TempDir::new().expect("create temp home");
        let codex_dir = temp_home.path().join(".codex");
        fs::create_dir_all(&codex_dir).expect("create codex dir");
        set_test_home_override(Some(temp_home.path()));
        let _settings = SettingsGuard::with_codex_config_dir(Some(codex_dir.to_str().unwrap()));
        let _env = CodexHomeEnvGuard::new(None);

        write_json_file(
            &codex_dir.join("models_cache.json"),
            &json!({ "models": [test_codex_model_template()] }),
        )
        .expect("seed model cache");

        let settings = json!({
            "auth": { "OPENAI_API_KEY": "sk-test" },
            "config": r#"model_provider = "vendor"
model_context_window = 128000

[model_providers.vendor]
base_url = "https://vendor.example/v1"
wire_api = "responses"
"#,
            "modelCatalog": {
                "models": [
                    {
                        "model": "deepseek-v4-flash",
                        "displayName": "DeepSeek V4 Flash",
                        "contextWindow": "64000"
                    }
                ]
            }
        });
        let auth = settings.get("auth").expect("auth");
        let config_text = settings.get("config").and_then(Value::as_str);

        write_codex_provider_live_with_catalog(&settings, Some("custom"), auth, config_text)
            .expect("write Codex live config with catalog");

        let live_config = fs::read_to_string(get_codex_config_path()).expect("read config.toml");
        let parsed: toml::Value = toml::from_str(&live_config).expect("parse config.toml");
        assert_eq!(
            parsed
                .get("model_catalog_json")
                .and_then(toml::Value::as_str),
            Some(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME)
        );
        let generated_catalog: Value = serde_json::from_str(
            &fs::read_to_string(get_codex_model_catalog_path()).expect("read catalog"),
        )
        .expect("parse generated catalog");
        let model = generated_catalog
            .get("models")
            .and_then(Value::as_array)
            .and_then(|models| models.first())
            .expect("generated model");
        assert_eq!(
            model.get("slug").and_then(Value::as_str),
            Some("deepseek-v4-flash")
        );
        assert_eq!(
            model.get("display_name").and_then(Value::as_str),
            Some("DeepSeek V4 Flash")
        );
        assert_eq!(
            model.get("context_window").and_then(Value::as_u64),
            Some(64_000)
        );

        let live_settings =
            read_codex_live_settings_with_model_catalog().expect("read live settings");
        assert_eq!(
            live_settings
                .pointer("/modelCatalog/models/0/model")
                .and_then(Value::as_str),
            Some("deepseek-v4-flash")
        );

        set_test_home_override(None);
    }

    #[test]
    fn restore_backfill_moves_bearer_token_back_to_auth() {
        let mut live_settings = json!({
            "auth": {
                "auth_mode": "chatgpt",
                "tokens": {
                    "access_token": "oauth-access"
                }
            },
            "config": r#"model_provider = "vendor_alpha"
model = "gpt-5.4"

[model_providers.vendor_alpha]
name = "Vendor Alpha"
base_url = "https://alpha.example/v1"
wire_api = "responses"
experimental_bearer_token = "sk-live"
"#
        });
        let template_settings = json!({
            "auth": {
                "OPENAI_API_KEY": "sk-template"
            }
        });

        restore_codex_settings_for_backfill(&mut live_settings, &template_settings, true)
            .expect("restore settings");
        assert_eq!(
            live_settings
                .get("auth")
                .and_then(|auth| auth.get("OPENAI_API_KEY"))
                .and_then(Value::as_str),
            Some("sk-live")
        );
        let config_text = live_settings
            .get("config")
            .and_then(Value::as_str)
            .expect("config text");
        assert!(
            !config_text.contains("experimental_bearer_token"),
            "stored provider config should not keep live bearer tokens"
        );
    }

    #[test]
    fn restore_backfill_preserves_model_catalog_from_template() {
        let mut live_settings = json!({
            "auth": {
                "OPENAI_API_KEY": "sk-live"
            },
            "config": "model = \"gpt-5.4\"\n"
        });
        let template_settings = json!({
            "auth": {
                "OPENAI_API_KEY": "sk-template"
            },
            "modelCatalog": {
                "models": [
                    {
                        "model": "deepseek-v4-flash",
                        "displayName": "DeepSeek V4 Flash"
                    }
                ]
            }
        });

        restore_codex_settings_for_backfill(&mut live_settings, &template_settings, true)
            .expect("restore settings");
        assert_eq!(
            live_settings
                .pointer("/modelCatalog/models/0/model")
                .and_then(Value::as_str),
            Some("deepseek-v4-flash")
        );
    }

    #[test]
    fn should_not_restore_provider_token_for_oauth_only_template() {
        let oauth_template = json!({
            "auth": {
                "auth_mode": "chatgpt",
                "tokens": {
                    "access_token": "oauth-access"
                }
            }
        });
        let api_key_template = json!({
            "auth": {
                "OPENAI_API_KEY": "sk-test"
            }
        });

        assert!(
            !should_restore_codex_provider_token_for_backfill(Some("custom"), &oauth_template),
            "OAuth-only templates should not backfill bearer tokens into OPENAI_API_KEY"
        );
        assert!(
            should_restore_codex_provider_token_for_backfill(Some("custom"), &api_key_template),
            "custom API-key providers should still restore provider bearer tokens"
        );
        assert!(
            !should_restore_codex_provider_token_for_backfill(Some("official"), &api_key_template),
            "official providers should never restore third-party bearer tokens"
        );
    }
}
