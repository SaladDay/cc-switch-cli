//! Grok Build (`~/.grok/config.toml`) 配置适配层。
//!
//! 供应商快照形状：`{"config": "<完整 config.toml 文本>"}`。官方条目
//! （category = "official"）的 config 为空——不写自定义模型表，Grok CLI
//! 回落到自带的 xAI OAuth 登录；非官方供应商必须携带完整的
//! `[models]` + `[model.<name>]` 自定义模型配置。

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

use crate::config::get_home_dir;
use crate::error::AppError;
use crate::provider::Provider;

pub const DEFAULT_MODEL: &str = "grok-4.7";
pub const DEFAULT_API_BACKEND: &str = "responses";
pub const DEFAULT_CONTEXT_WINDOW: i64 = 500_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrokModelConfig {
    pub profile: String,
    pub model: String,
    pub base_url: String,
    pub name: String,
    pub api_key: Option<String>,
    pub env_key: Option<String>,
    pub api_backend: String,
    pub context_window: i64,
}

/// Grok Build 配置目录（`~/.grok`）。
pub fn get_grok_config_dir() -> PathBuf {
    crate::settings::get_grok_override_dir().unwrap_or_else(|| get_home_dir().join(".grok"))
}

/// Grok Build live 配置路径（`~/.grok/config.toml`）。
pub fn get_grok_config_path() -> PathBuf {
    get_grok_config_dir().join("config.toml")
}

fn required_non_empty_string<'a>(
    table: &'a toml::value::Table,
    key: &str,
) -> Result<&'a str, AppError> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::localized(
                "provider.grokbuild.field.missing",
                format!("Grok Build 配置缺少有效的 {key} 字段"),
                format!("Grok Build configuration is missing a valid {key} field"),
            )
        })
}

fn optional_non_empty_string(table: &toml::value::Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// 仅做语法校验的 Grok Build config 文档校验（允许空文档）。
///
/// 官方条目走 Grok CLI 自带的 xAI OAuth 登录，config.toml 不需要（通常也没有）
/// 自定义模型表：空文档合法，非空只要求 TOML 语法合法。live 层的读写与官方
/// 快照校验都用它；"必须有完整自定义模型表"的强校验见 `validate_config_toml`。
pub fn validate_config_toml_syntax(config_toml: &str) -> Result<(), AppError> {
    if config_toml.trim().is_empty() {
        return Ok(());
    }
    config_toml
        .parse::<toml::Value>()
        .map(|_| ())
        .map_err(|error| {
            AppError::localized(
                "provider.grokbuild.config.invalid_toml",
                format!("Grok Build config.toml 格式错误: {error}"),
                format!("Invalid Grok Build config.toml: {error}"),
            )
        })
}

/// Official login may still have global [models] preferences. Incomplete explicit
/// custom profiles must not be silently reclassified as official.
pub fn is_official_live_config(config_toml: &str) -> bool {
    let Ok(document) = config_toml.parse::<toml::Value>() else {
        return false;
    };
    let Some(root) = document.as_table() else {
        return false;
    };
    !root.contains_key("model")
        && root.get("models").is_none_or(|models| {
            models
                .as_table()
                .is_some_and(|table| !table.contains_key("default"))
        })
}

/// 校验供应商自有的 Grok Build TOML 文档。
pub fn validate_config_toml(config_toml: &str) -> Result<(), AppError> {
    let document = config_toml.parse::<toml::Value>().map_err(|error| {
        AppError::localized(
            "provider.grokbuild.config.invalid_toml",
            format!("Grok Build config.toml 格式错误: {error}"),
            format!("Invalid Grok Build config.toml: {error}"),
        )
    })?;

    let root = document.as_table().ok_or_else(|| {
        AppError::localized(
            "provider.grokbuild.config.not_table",
            "Grok Build 配置必须是 TOML 表结构",
            "Grok Build configuration must be a TOML table",
        )
    })?;
    let models = root
        .get("models")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| {
            AppError::localized(
                "provider.grokbuild.models.missing",
                "Grok Build 配置缺少 [models]",
                "Grok Build configuration is missing [models]",
            )
        })?;
    let default_model = required_non_empty_string(models, "default")?;
    let model_entries = root
        .get("model")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| {
            AppError::localized(
                "provider.grokbuild.model.missing",
                "Grok Build 配置缺少 [model.<name>]",
                "Grok Build configuration is missing [model.<name>]",
            )
        })?;
    let selected_model = model_entries
        .get(default_model)
        .and_then(toml::Value::as_table)
        .ok_or_else(|| {
            AppError::localized(
                "provider.grokbuild.default_model.missing",
                format!("Grok Build 配置缺少 [model.\"{default_model}\"]"),
                format!("Grok Build configuration is missing [model.\"{default_model}\"]"),
            )
        })?;

    required_non_empty_string(selected_model, "model")?;
    required_non_empty_string(selected_model, "base_url")?;
    required_non_empty_string(selected_model, "name")?;
    if optional_non_empty_string(selected_model, "api_key").is_none()
        && optional_non_empty_string(selected_model, "env_key").is_none()
    {
        return Err(AppError::localized(
            "provider.grokbuild.credentials.missing",
            "Grok Build 配置缺少有效的 api_key 或 env_key 字段",
            "Grok Build configuration is missing a valid api_key or env_key field",
        ));
    }
    let backend = required_non_empty_string(selected_model, "api_backend")?;
    if !["responses", "chat_completions", "messages"].contains(&backend) {
        return Err(AppError::InvalidInput(
            "Grok api_backend must be responses, chat_completions, or messages".into(),
        ));
    }

    selected_model
        .get("context_window")
        .and_then(toml::Value::as_integer)
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            AppError::localized(
                "provider.grokbuild.context_window.invalid",
                "Grok Build context_window 必须是正整数",
                "Grok Build context_window must be a positive integer",
            )
        })?;

    Ok(())
}

pub fn extract_model_config(config_toml: &str) -> Option<GrokModelConfig> {
    let document = config_toml.parse::<toml::Value>().ok()?;
    let root = document.as_table()?;
    let default_model = root
        .get("models")?
        .as_table()?
        .get("default")?
        .as_str()?
        .trim();
    let selected_model = root
        .get("model")?
        .as_table()?
        .get(default_model)?
        .as_table()?;
    Some(GrokModelConfig {
        profile: default_model.to_string(),
        model: selected_model.get("model")?.as_str()?.trim().to_string(),
        base_url: selected_model
            .get("base_url")?
            .as_str()?
            .trim_end_matches('/')
            .to_string(),
        name: selected_model.get("name")?.as_str()?.trim().to_string(),
        api_key: optional_non_empty_string(selected_model, "api_key"),
        env_key: optional_non_empty_string(selected_model, "env_key"),
        api_backend: selected_model
            .get("api_backend")?
            .as_str()?
            .trim()
            .to_string(),
        context_window: selected_model.get("context_window")?.as_integer()?,
    })
}

pub fn extract_credentials(config_toml: &str) -> Option<(String, String)> {
    let config = extract_model_config(config_toml)?;
    // 凭据只来自两个显式声明的来源：
    //   1. 配置里的内联 `api_key`，或
    //   2. `env_key` 指名的进程环境变量。
    //
    // 刻意不无条件兜底 `XAI_API_KEY`：声明的 `env_key` 变量未设置时，静默换上
    // 别家账号的 key 会把密钥泄漏给这份 config 指向的任意 `base_url`。声明了
    // 间接引用但变量缺失必须呈现为"没有凭据"（None），让调用方大声失败，
    // 而不是把错误的密钥发出去。
    let api_key = config.api_key.or_else(|| {
        config
            .env_key
            .as_deref()
            .and_then(|key| std::env::var(key).ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })?;
    Some((config.base_url, api_key))
}

/// Use a private live-only profile name so routing Official cannot overwrite a
/// user's model table whose key happens to be the built-in model ID.
pub(crate) fn official_proxy_settings(
    live: &str,
    proxy_url: &str,
    placeholder: &str,
) -> Result<Value, AppError> {
    let current = live
        .parse::<toml::Value>()
        .map_err(|error| AppError::Config(error.to_string()))?;
    let base = "cc-switch-grok-official";
    let selected = selected_model_profile(live);
    let mut profile = selected
        .as_deref()
        .filter(|name| name.starts_with(base) && has_proxy_placeholder(live, placeholder))
        .unwrap_or(base)
        .to_owned();
    let mut suffix = 1;
    while current.get("model").and_then(|v| v.get(&profile)).is_some()
        && !(selected.as_deref() == Some(profile.as_str())
            && has_proxy_placeholder(live, placeholder))
    {
        suffix += 1;
        profile = format!("{base}-{suffix}");
    }
    let settings = build_provider_settings_config(
        "Grok Official (CC-Switch)",
        proxy_url,
        placeholder,
        DEFAULT_MODEL,
        "responses",
    );
    let mut doc = settings["config"]
        .as_str()
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| AppError::Config(error.to_string()))?;
    let table = doc["model"]
        .as_table_like_mut()
        .unwrap()
        .remove(DEFAULT_MODEL)
        .unwrap();
    doc["models"]["default"] = toml_edit::value(&profile);
    doc["model"][&profile] = table;
    Ok(json!({"config":doc.to_string()}))
}

pub fn extract_inline_api_key(config_toml: &str) -> Option<String> {
    extract_model_config(config_toml)?.api_key
}

pub fn extract_base_url(config_toml: &str) -> Option<String> {
    Some(extract_model_config(config_toml)?.base_url)
}

/// 规范化 `--api-format` 到 Grok Build 的 `api_backend` 取值。
pub fn normalize_api_backend(raw: &str) -> Result<&'static str, AppError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" => Ok(DEFAULT_API_BACKEND),
        "responses" | "openai_responses" | "openai-responses" => Ok("responses"),
        "chat" | "chat_completions" | "chat-completions" | "openai_chat" | "openai-chat" => {
            Ok("chat_completions")
        }
        "anthropic" | "anthropic_messages" | "anthropic-messages" | "messages" => Ok("messages"),
        other => Err(AppError::InvalidInput(format!(
            "invalid API format '{other}'. Valid values: responses, chat, messages"
        ))),
    }
}

/// 从端点/密钥/模型组装供应商快照（非交互 add 与 deeplink 导入共用此形状）。
/// profile 名与上游模型 id 一致，与 GUI 的导入行为相同。
pub fn build_provider_settings_config(
    name: &str,
    base_url: &str,
    api_key: &str,
    model: &str,
    api_backend: &str,
) -> Value {
    let quote = |value: &str| toml_edit::Value::from(value).to_string();
    let model = model.trim();
    let model = if model.is_empty() {
        DEFAULT_MODEL
    } else {
        model
    };
    let backend = if api_backend.trim().is_empty() {
        DEFAULT_API_BACKEND
    } else {
        api_backend.trim()
    };
    let config = format!(
        "[models]
default = {model_q}

[model.{model_q}]
model = {model_q}
base_url = {endpoint_q}
name = {name_q}
api_key = {key_q}
api_backend = {backend_q}
context_window = {context_window}
",
        model_q = quote(model),
        endpoint_q = quote(base_url.trim()),
        name_q = quote(name.trim()),
        key_q = quote(api_key.trim()),
        backend_q = quote(backend),
        context_window = DEFAULT_CONTEXT_WINDOW,
    );
    json!({ "config": config })
}

/// Update the selected model without dropping its profile name, env_key, headers,
/// context window, or other client-supported options.
pub(crate) fn update_provider_settings_config(
    current: Option<&Value>,
    name: &str,
    base_url: &str,
    api_key: &str,
    model: &str,
    backend: &str,
) -> Value {
    let Some(text) = current
        .and_then(|v| v.get("config"))
        .and_then(Value::as_str)
    else {
        return build_provider_settings_config(name, base_url, api_key, model, backend);
    };
    let Some(selected) = extract_model_config(text) else {
        return build_provider_settings_config(name, base_url, api_key, model, backend);
    };
    let Ok(mut doc) = text.parse::<toml_edit::DocumentMut>() else {
        return current.unwrap().clone();
    };
    for (field, value) in [
        ("name", name),
        ("base_url", base_url),
        ("model", model),
        ("api_backend", backend),
    ] {
        let value = if field == "api_backend" && value.trim().is_empty() {
            DEFAULT_API_BACKEND
        } else {
            value.trim()
        };
        doc["model"][&selected.profile][field] = toml_edit::value(value);
    }
    if api_key.trim().is_empty() {
        if let Some(table) = doc["model"][&selected.profile].as_table_like_mut() {
            table.remove("api_key");
        }
    } else {
        doc["model"][&selected.profile]["api_key"] = toml_edit::value(api_key.trim());
    }
    json!({ "config": doc.to_string() })
}

fn update_selected_model_string(
    config_toml: &str,
    field: &str,
    value: &str,
) -> Result<String, AppError> {
    let mut document = config_toml
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| {
            AppError::localized(
                "provider.grokbuild.config.invalid_toml",
                format!("Grok Build config.toml 格式错误: {error}"),
                format!("Invalid Grok Build config.toml: {error}"),
            )
        })?;
    let default_model = document
        .get("models")
        .and_then(|item| item.get("default"))
        .and_then(toml_edit::Item::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| {
            AppError::localized(
                "provider.grokbuild.default_model.missing",
                "Grok Build 配置缺少 models.default",
                "Grok Build configuration is missing models.default",
            )
        })?
        .to_string();

    let selected_model = document
        .get_mut("model")
        .and_then(|item| item.get_mut(&default_model))
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or_else(|| {
            AppError::localized(
                "provider.grokbuild.default_model.missing",
                format!("Grok Build 配置缺少 [model.\"{default_model}\"]"),
                format!("Grok Build configuration is missing [model.\"{default_model}\"]"),
            )
        })?;
    selected_model.insert(field, toml_edit::value(value));
    Ok(document.to_string())
}

pub fn apply_proxy_takeover(
    config_toml: &str,
    proxy_base_url: &str,
    token_placeholder: &str,
) -> Result<String, AppError> {
    let updated = update_selected_model_string(config_toml, "base_url", proxy_base_url)?;
    let updated = update_selected_model_string(&updated, "api_key", token_placeholder)?;
    update_selected_model_string(&updated, "api_backend", DEFAULT_API_BACKEND)
}

/// Profile discovery does not depend on optional/native model fields.
pub(crate) fn selected_model_profile(text: &str) -> Option<String> {
    let doc = text.parse::<toml::Value>().ok()?;
    let name = doc.get("models")?.get("default")?.as_str()?;
    if name.trim().is_empty() || !doc.get("model")?.get(name)?.is_table() {
        return None;
    }
    Some(name.to_owned())
}

pub fn has_proxy_placeholder(config_toml: &str, token_placeholder: &str) -> bool {
    let Some(profile) = selected_model_profile(config_toml) else {
        return false;
    };
    config_toml
        .parse::<toml::Value>()
        .ok()
        .and_then(|doc| {
            doc.get("model")?
                .get(&profile)?
                .get("api_key")?
                .as_str()
                .map(str::to_owned)
        })
        .is_some_and(|key| key == token_placeholder)
}

/// 读取 live `~/.grok/config.toml`，返回供应商 settings 快照。
///
/// 只做 TOML 语法校验：live 处于官方态（无自定义模型表）时同样需要能被
/// 读取，供切换回填与界面展示使用。需要"完整自定义模型配置"的导入路径
/// 由调用方自行叠加 `validate_config_toml`。
pub fn read_grok_live_settings() -> Result<Value, AppError> {
    let path = get_grok_config_path();
    if !path.exists() {
        return Err(AppError::localized(
            "grokbuild.config.missing",
            "Grok Build 配置文件不存在",
            "Grok Build configuration file not found",
        ));
    }
    let config = fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?;
    validate_config_toml_syntax(&config)?;
    Ok(json!({ "config": config }))
}

/// 把供应商快照写进 live `~/.grok/config.toml`。
///
/// 官方条目不注入自定义模型表：按快照原样写回（首次为空文件），
/// Grok CLI 回落到官方内置模型 + 自带 OAuth 登录；MCP 投影随后由
/// 切换流程重新补写。非官方供应商必须携带完整的自定义模型配置。
/// Project only routing fields. Global settings, MCP, and unrelated custom models
/// belong to Grok/the user and must never be replayed from a provider snapshot.
/// 在应用锁内构建补丁所需的全部静态输入：校验、选中 profile 与要写入的表。
/// 与 durable `Written` 状态相关的部分刻意留到 [`build_patch_under_lock`]。
#[derive(Clone)]
pub(crate) struct GrokPatchInputs {
    pub(crate) official: bool,
    pub(crate) profile: String,
    /// 选中模型的整张表（含 env_key、extra_headers 等全部选项）。
    pub(crate) model_table: toml_edit::Item,
    /// 首次写入时是否把 live 当前选中的表纳入清理记录。只用于从 live 自身
    /// 派生的写入（代理接管/恢复：原始内容保存在接管备份里）；普通切换不得
    /// 采用，否则用户手写且未入库的表会被删除且无处恢复。
    pub(crate) adopt_selected_on_first_write: bool,
}

impl GrokPatchInputs {
    pub(crate) fn of(provider: &Provider) -> Result<Self, AppError> {
        let text = provider
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AppError::InvalidInput("Grok Build config must be a TOML string".into())
            })?;
        let official = provider.category.as_deref() == Some("official");
        if official {
            validate_config_toml_syntax(text)?;
        } else {
            validate_config_toml(text)?;
        }
        let (profile, model_table) = if official {
            (String::new(), toml_edit::Item::None)
        } else {
            let doc = text
                .parse::<toml_edit::DocumentMut>()
                .map_err(|e| AppError::Config(e.to_string()))?;
            let model = extract_model_config(text)
                .ok_or_else(|| AppError::InvalidInput("Missing Grok model profile".into()))?;
            let model_table = doc["model"][&model.profile].clone();
            (model.profile, model_table)
        };
        Ok(Self {
            official,
            profile,
            model_table,
            adopt_selected_on_first_write: false,
        })
    }
}

/// 以语义值比较两个模型表（忽略格式与注释差异）。
fn table_value(item: &toml_edit::Item) -> Option<toml::Value> {
    let mut doc = toml_edit::DocumentMut::new();
    doc["t"] = item.clone();
    toml::from_str::<toml::Value>(&doc.to_string())
        .ok()?
        .get("t")
        .cloned()
}

/// 构建 TOML 补丁。必须在持有 grokbuild 应用锁时调用：remove 列表来自
/// durable `Written` 记录（外加首写时对 live 选中表的采用），跨进程并发
/// 切换下晚到的一方必须在锁内重读，否则会互相覆盖清理记录。
pub(crate) fn build_patch_under_lock(
    inputs: &GrokPatchInputs,
) -> Result<
    (
        crate::live::patch::toml::TomlPatch,
        crate::mode::state::Written,
    ),
    AppError,
> {
    use crate::live::patch::{toml::TomlPatch, KeyPath};
    let store = crate::live::engine::DeviceStore::for_device();
    let previous = crate::mode::state::written(&store, "grokbuild")?;
    let first_write = previous.is_none();
    let mut tables = previous.map(|w| w.tables).unwrap_or_default();
    let path = get_grok_config_path();
    let live = if path.exists() {
        let live = fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
        validate_config_toml_syntax(&live)?;
        Some(live)
    } else {
        None
    };
    // On the first switch, adopt the selected live custom table for cleanup.
    // Later switches use the durable record, not models.default, which Grok may edit.
    if first_write && inputs.adopt_selected_on_first_write {
        if let Some(profile) = live.as_deref().and_then(selected_model_profile) {
            tables.push(profile);
        }
    }
    // 不覆盖用户自己写的同名模型表：它不在 Written 记录里，覆盖后下次切换
    // 还会被当作 CC-Switch 的表删除。内容完全相同则视为同一份配置。
    if !inputs.official && !tables.contains(&inputs.profile) {
        if let Some(existing) = live
            .as_deref()
            .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
            .and_then(|doc| doc.get("model")?.get(&inputs.profile).cloned())
        {
            if table_value(&existing) != table_value(&inputs.model_table) {
                return Err(AppError::localized(
                    "provider.grokbuild.profile.collision",
                    format!(
                        "~/.grok/config.toml 中已有用户定义的模型表 \"{}\"，为避免覆盖已停止切换；请为该供应商换一个配置名",
                        inputs.profile
                    ),
                    format!(
                        "~/.grok/config.toml already has a user-defined model table \"{}\"; switching stopped to avoid overwriting it. Rename this provider's profile",
                        inputs.profile
                    ),
                ));
            }
        }
    }
    let mut patch = TomlPatch::default();
    patch.remove.push(KeyPath::new(&["models", "default"]));
    patch
        .remove
        .extend(tables.iter().map(|name| KeyPath::new(&["model", name])));
    let mut written = crate::mode::state::Written::default();
    if !inputs.official {
        patch.set.push((
            KeyPath::new(&["models", "default"]),
            toml_edit::value(&inputs.profile),
        ));
        // Preserve all options of the selected model, including headers/env_key.
        patch.set.push((
            KeyPath::new(&["model", &inputs.profile]),
            inputs.model_table.clone(),
        ));
        written.tables.push(inputs.profile.clone());
    }
    // A remove followed by set on the same key would lose comments/position.
    patch
        .remove
        .retain(|key| !patch.set.iter().any(|(target, _)| target == key));
    Ok((patch, written))
}

/// 原样写 live 文件的底层入口，与 `read_grok_live_settings` 对应（仅语法校验）。
///
/// 代理接管的备份/恢复也走这里：官方态 live（无自定义模型表）必须可以
/// 原样写回。完整形状校验由 `write_grok_provider_live` 的非官方分支负责。
pub fn write_grok_live_settings(settings: &Value) -> Result<(), AppError> {
    let config = settings
        .get("config")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AppError::localized(
                "provider.grokbuild.config.missing",
                "Grok Build 配置缺少 config 字段",
                "Grok Build configuration is missing the config field",
            )
        })?;
    validate_config_toml_syntax(config)?;
    crate::config::write_text_file(&get_grok_config_path(), config)
}

pub(crate) fn apply_provider_patch(
    inputs: &GrokPatchInputs,
    db_target: Option<(&crate::Database, Option<&str>)>,
) -> Result<(), AppError> {
    use crate::live::engine::{lock_app, DeviceStore, LiveFile};
    use crate::mode::{
        operation,
        state::{self, PendingTarget},
    };
    let store_root = DeviceStore::for_device();
    let path = store_root.file("grokbuild-write.lock");
    if let Some(parent) = path.parent() {
        crate::config::create_managed_config_dir_all(parent)?;
    }
    let file_guard = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| AppError::io(&path, error))?;
    file_guard
        .lock()
        .map_err(|error| AppError::io(&path, error))?;
    if let Some((db, pointer)) = db_target {
        // AppWrite::begin 持锁并先补完上次未完成操作；补丁必须在锁内构建。
        let write = operation::AppWrite::begin(db, "grokbuild")?;
        let (patch, written) = build_patch_under_lock(inputs)?;
        let target = PendingTarget {
            pointer: pointer.map(str::to_owned),
            written: Some(written),
            ..Default::default()
        };
        let changes = [operation::FileChange {
            file: LiveFile::private(get_grok_config_path()),
            patch: &patch,
        }];
        write.run("switch", &changes, target)?;
    } else {
        let store = DeviceStore::for_device();
        let guard = lock_app("grokbuild");
        let (patch, written) = build_patch_under_lock(inputs)?;
        let target = PendingTarget {
            written: Some(written),
            ..Default::default()
        };
        let changes = [operation::FileChange {
            file: LiveFile::private(get_grok_config_path()),
            patch: &patch,
        }];
        let commit = |target: &PendingTarget| {
            if target.pointer.is_some() {
                return Err(AppError::Config(
                    "An unfinished Grok switch requires startup recovery".into(),
                ));
            }
            state::update(&store, |live| {
                live.apps.entry("grokbuild".into()).or_default().written = target.written.clone();
            })
        };
        operation::recover_before_write(&store, &guard, &commit)?;
        operation::run(&store, &guard, "apply", &changes, target, &commit)?;
    }
    Ok(())
}

pub fn write_grok_provider_live(provider: &Provider) -> Result<(), AppError> {
    let inputs = GrokPatchInputs::of(provider)?;
    apply_provider_patch(&inputs, None)
}

/// 代理接管/恢复专用：写入由 live 自身派生的快照，首次写入时采用当前选中表
/// （其原始内容已在接管备份中，恢复时会写回）。
pub(crate) fn write_grok_live_snapshot(provider: &Provider) -> Result<(), AppError> {
    let mut inputs = GrokPatchInputs::of(provider)?;
    inputs.adopt_selected_on_first_write = true;
    apply_provider_patch(&inputs, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    fn valid_config() -> &'static str {
        r#"[models]
default = "grok-4.5"

[model."grok-4.5"]
model = "grok-4.5"
base_url = "https://example.com/v1"
name = "Example"
api_key = "secret"
api_backend = "responses"
context_window = 500000
"#
    }

    fn valid_env_key_config() -> &'static str {
        r#"[models]
default = "grok-env"

[model."grok-env"]
model = "grok-4.5"
base_url = "https://example.com/v1"
name = "Example Env"
env_key = "GROK_TEST_API_KEY"
api_backend = "responses"
context_window = 500000
"#
    }

    #[test]
    fn grok_official_proxy_restore_preserves_native_auth_and_user_configuration() {
        let temp = TempDir::new().unwrap();
        let _env = crate::test_support::TestEnvGuard::isolated(temp.path());
        fs::create_dir_all(get_grok_config_dir()).unwrap();
        let native = b"native-grok-credentials-not-managed-by-switch";
        fs::write(get_grok_config_dir().join("auth.json"), native).unwrap();
        let original = "# native settings\n[ui]\ncompact_mode = true\n[model.\"grok-4.7\"]\nmodel = \"user-model\"\n[mcp_servers.echo]\ncommand = \"echo\"\n";
        fs::write(get_grok_config_path(), original).unwrap();
        let settings = official_proxy_settings(
            original,
            "http://127.0.0.1:15724/grokbuild/v1",
            "PROXY_MANAGED",
        )
        .unwrap();
        let profile = selected_model_profile(settings["config"].as_str().unwrap()).unwrap();
        let routed = Provider::with_id("live-only".into(), "Proxy".into(), settings, None);
        write_grok_provider_live(&routed).unwrap();
        let live: toml::Value = fs::read_to_string(get_grok_config_path())
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            live["model"][&profile]["api_key"].as_str(),
            Some("PROXY_MANAGED")
        );
        assert_eq!(
            live["model"]["grok-4.7"]["model"].as_str(),
            Some("user-model")
        );
        let mut official = Provider::with_id(
            "grokbuild-official".into(),
            "Grok Official".into(),
            json!({"config":""}),
            None,
        );
        official.category = Some("official".into());
        write_grok_provider_live(&official).unwrap();
        let text = fs::read_to_string(get_grok_config_path()).unwrap();
        let restored: toml::Value = text.parse().unwrap();
        assert!(!text.contains("PROXY_MANAGED"));
        assert!(restored["model"].get(&profile).is_none());
        assert_eq!(
            restored["model"]["grok-4.7"]["model"].as_str(),
            Some("user-model")
        );
        assert_eq!(
            restored["mcp_servers"]["echo"]["command"].as_str(),
            Some("echo")
        );
        assert_eq!(restored["ui"]["compact_mode"].as_bool(), Some(true));
        assert_eq!(
            fs::read(get_grok_config_dir().join("auth.json")).unwrap(),
            native
        );
    }

    #[test]
    fn official_proxy_profile_does_not_overwrite_user_model_tables() {
        let live = "[model.\"grok-4.7\"]\nmodel = \"user-model\"\n[model.cc-switch-grok-official]\nmodel = \"also-user\"\n";
        let settings =
            official_proxy_settings(live, "http://127.0.0.1:15724/grokbuild/v1", "PROXY_MANAGED")
                .unwrap();
        let text = settings["config"].as_str().unwrap();
        let profile = selected_model_profile(text).unwrap();
        assert_eq!(profile, "cc-switch-grok-official-2");
        let doc: toml::Value = text.parse().unwrap();
        assert_eq!(doc["model"][&profile]["model"].as_str(), Some("grok-4.7"));
        assert_eq!(
            official_proxy_settings(text, "http://127.0.0.1:15724/grokbuild/v1", "PROXY_MANAGED")
                .unwrap()["config"],
            text
        );
    }

    #[test]
    fn validates_expected_config_shape() {
        validate_config_toml(valid_config()).expect("valid Grok Build config");
        validate_config_toml(valid_env_key_config()).expect("valid env_key configuration");
    }

    #[test]
    fn syntax_validation_accepts_official_snapshots() {
        validate_config_toml_syntax("").expect("empty official snapshot");
        validate_config_toml_syntax("[mcp_servers.echo]\ncommand = \"echo\"\n")
            .expect("official-mode config without model tables");
        assert!(validate_config_toml_syntax("not = [valid").is_err());
    }

    #[test]
    fn official_live_config_detection() {
        // 官方态：完全没有自定义模型痕迹
        assert!(is_official_live_config(""));
        assert!(is_official_live_config("  \n# comment only\n"));
        assert!(is_official_live_config(
            "[mcp_servers.echo]\ncommand = \"echo\"\n"
        ));

        // 出现过任一自定义键（哪怕残缺）都不是官方态，交给强校验报错
        assert!(!is_official_live_config(valid_config()));
        assert!(!is_official_live_config("[models]\ndefault = \"x\"\n"));
        assert!(!is_official_live_config("[model.x]\nmodel = \"x\"\n"));

        // 语法不合法不是官方态
        assert!(!is_official_live_config("not = [valid"));
    }

    #[test]
    fn rejects_missing_selected_model_table() {
        let error = validate_config_toml("[models]\ndefault = \"grok-4.5\"\n")
            .expect_err("missing model table should fail");
        assert!(error.to_string().contains("model"));
    }

    #[test]
    fn rejects_config_without_api_key_or_env_key() {
        let config = valid_config().replace("api_key = \"secret\"\n", "");
        let error = validate_config_toml(&config).expect_err("credentials should be required");
        assert!(error.to_string().contains("api_key"));
        assert!(error.to_string().contains("env_key"));
    }

    #[test]
    fn extracts_selected_model_and_updates_takeover_fields() {
        let selected = extract_model_config(valid_config()).expect("selected model");
        assert_eq!(selected.profile, "grok-4.5");
        assert_eq!(selected.model, "grok-4.5");
        assert_eq!(selected.base_url, "https://example.com/v1");

        let updated = apply_proxy_takeover(
            valid_config(),
            "http://127.0.0.1:15721/grokbuild/v1",
            "PROXY_MANAGED",
        )
        .expect("takeover config");
        let selected = extract_model_config(&updated).expect("updated selected model");
        assert_eq!(selected.base_url, "http://127.0.0.1:15721/grokbuild/v1");
        assert_eq!(selected.api_key.as_deref(), Some("PROXY_MANAGED"));
        assert!(has_proxy_placeholder(&updated, "PROXY_MANAGED"));
    }

    #[test]
    fn takeover_preserves_env_key_profile_and_injects_inline_placeholder() {
        let direct_config = valid_env_key_config().replace(
            "api_backend = \"responses\"",
            "api_backend = \"chat_completions\"",
        );
        let updated = apply_proxy_takeover(
            &direct_config,
            "http://127.0.0.1:15721/grokbuild/v1",
            "PROXY_MANAGED",
        )
        .expect("takeover config");
        let selected = extract_model_config(&updated).expect("updated selected model");

        assert_eq!(selected.profile, "grok-env");
        assert_eq!(selected.env_key.as_deref(), Some("GROK_TEST_API_KEY"));
        assert_eq!(selected.api_key.as_deref(), Some("PROXY_MANAGED"));
        assert_eq!(selected.api_backend, DEFAULT_API_BACKEND);
    }

    #[test]
    #[serial]
    fn resolves_api_key_from_configured_environment_variable() {
        let original = std::env::var_os("GROK_TEST_API_KEY");
        std::env::set_var("GROK_TEST_API_KEY", "env-secret");

        let credentials = extract_credentials(valid_env_key_config()).expect("credentials");

        assert_eq!(credentials.0, "https://example.com/v1");
        assert_eq!(credentials.1, "env-secret");
        match original {
            Some(value) => std::env::set_var("GROK_TEST_API_KEY", value),
            None => std::env::remove_var("GROK_TEST_API_KEY"),
        }
    }

    /// 构造一个 `env_key` 指向未设置环境变量的 config——这是"声明了间接引用但
    /// 该变量不存在"的场景，修复前会静默兜底到 `XAI_API_KEY`。
    fn env_key_unset_config() -> &'static str {
        r#"[models]
default = "grok-env"

[model."grok-env"]
model = "grok-4.5"
base_url = "https://attacker.example/v1"
name = "Attacker Env"
env_key = "GROK_TEST_DEFINITELY_UNSET_VAR"
api_backend = "responses"
context_window = 500000
"#
    }

    #[test]
    #[serial]
    fn does_not_fall_back_to_xai_api_key_when_declared_env_key_is_unset() {
        // 即使进程里恰好设了 XAI_API_KEY，也不能被静默借用到别的 base_url 上。
        let original_xai = std::env::var_os("XAI_API_KEY");
        let original_unset = std::env::var_os("GROK_TEST_DEFINITELY_UNSET_VAR");
        std::env::set_var("XAI_API_KEY", "xai-secret-should-not-leak");
        std::env::remove_var("GROK_TEST_DEFINITELY_UNSET_VAR");

        let credentials = extract_credentials(env_key_unset_config());

        assert!(
            credentials.is_none(),
            "declared env_key unset must yield None, never a borrowed XAI_API_KEY; got {credentials:?}"
        );

        match original_xai {
            Some(value) => std::env::set_var("XAI_API_KEY", value),
            None => std::env::remove_var("XAI_API_KEY"),
        }
        match original_unset {
            Some(value) => std::env::set_var("GROK_TEST_DEFINITELY_UNSET_VAR", value),
            None => std::env::remove_var("GROK_TEST_DEFINITELY_UNSET_VAR"),
        }
    }

    #[test]
    #[serial]
    fn official_provider_roundtrips_without_custom_model_tables() {
        let temp = TempDir::new().expect("temp dir");
        let _guard = crate::test_support::TestEnvGuard::isolated(temp.path());

        // 官方条目：空 config 可写。live 里没有自定义模型表时补丁为空，
        // 但仍会落一个空 config.toml（与 GUI 行为一致，Grok 视同无自定义模型）。
        let mut official = Provider::with_id(
            "grokbuild-official".to_string(),
            "Grok Official".to_string(),
            json!({ "config": "" }),
            None,
        );
        official.category = Some("official".to_string());
        write_grok_provider_live(&official).expect("official empty config is writable");
        assert_eq!(
            fs::read_to_string(get_grok_config_path()).expect("read config"),
            ""
        );

        // 先通过供应商写入建立管理记录，再验证切回官方只清掉受管理的模型。
        fs::write(get_grok_config_path(), "[ui]\ncompact_mode = true\n")
            .expect("seed user preferences");
        let custom = Provider::with_id(
            "third-party".into(),
            "Third Party".into(),
            json!({ "config": valid_config() }),
            None,
        );
        write_grok_provider_live(&custom).expect("write managed profile");
        write_grok_provider_live(&official).expect("official switch over managed profile");
        let restored: toml::Value = fs::read_to_string(get_grok_config_path())
            .expect("read restored config")
            .parse()
            .expect("parse restored config");
        assert!(restored
            .get("model")
            .and_then(|v| v.get("grok-4.5"))
            .is_none());
        assert_eq!(restored["ui"]["compact_mode"].as_bool(), Some(true));

        // 官方态 live（如 MCP 投影补写后）无自定义模型表，读取与原样写回都必须可用
        let official_live = "[mcp_servers.echo]\ncommand = \"echo\"\n";
        write_grok_live_settings(&json!({ "config": official_live }))
            .expect("official-mode live is writable for backup restore");
        let settings = read_grok_live_settings().expect("official-mode live is readable");
        assert_eq!(
            settings.get("config").and_then(Value::as_str),
            Some(official_live)
        );

        // 非官方供应商仍要求完整的自定义模型配置
        let custom = Provider::with_id(
            "custom".to_string(),
            "Custom".to_string(),
            json!({ "config": "" }),
            None,
        );
        assert!(write_grok_provider_live(&custom).is_err());
    }

    #[test]
    #[serial]
    fn writes_and_reads_live_config() {
        let temp = TempDir::new().expect("temp dir");
        let _guard = crate::test_support::TestEnvGuard::isolated(temp.path());

        let provider = Provider::with_id(
            "grok".to_string(),
            "Example".to_string(),
            json!({ "config": valid_config() }),
            None,
        );
        write_grok_provider_live(&provider).expect("write live config");

        let path = get_grok_config_path();
        assert_eq!(path, temp.path().join(".grok").join("config.toml"));
        assert_eq!(
            fs::read_to_string(path).expect("read config"),
            valid_config()
        );
        assert_eq!(
            read_grok_live_settings()
                .expect("read live settings")
                .get("config")
                .and_then(Value::as_str),
            Some(valid_config())
        );
    }
}
