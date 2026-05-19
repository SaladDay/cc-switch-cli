// unused imports removed
use std::path::PathBuf;

use crate::config::{
    atomic_write, delete_file, home_dir, sanitize_provider_name, write_json_file, write_text_file,
};
use crate::error::AppError;
use serde_json::Value;
use std::fs;
use std::path::Path;
use toml_edit::DocumentMut;

pub const CC_SWITCH_CODEX_MODEL_PROVIDER_ID: &str = "ccswitch";

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
///
/// Priority: `CODEX_HOME` env var > cc-switch settings override > `$HOME/.codex`
pub fn get_codex_config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CODEX_HOME") {
        let dir = PathBuf::from(dir);
        if !dir.as_os_str().is_empty() && !dir.to_string_lossy().trim().is_empty() {
            return expand_codex_config_dir(dir);
        }
    }

    if let Some(custom) = crate::settings::get_codex_override_dir() {
        return custom;
    }

    home_dir().expect("无法获取用户主目录").join(".codex")
}

fn expand_codex_config_dir(path: PathBuf) -> PathBuf {
    let lossy = path.to_string_lossy();
    if lossy == "~" {
        return home_dir().unwrap_or(path);
    }

    if let Some(rest) = lossy
        .strip_prefix("~/")
        .or_else(|| lossy.strip_prefix("~\\"))
    {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }

    path
}

/// 获取 Codex auth.json 路径
pub fn get_codex_auth_path() -> PathBuf {
    get_codex_config_dir().join("auth.json")
}

/// 获取 Codex config.toml 路径
pub fn get_codex_config_path() -> PathBuf {
    get_codex_config_dir().join("config.toml")
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

fn is_custom_codex_model_provider_id(id: &str) -> bool {
    let id = id.trim();
    !id.is_empty()
        && !CODEX_RESERVED_MODEL_PROVIDER_IDS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(id))
}

fn stable_codex_model_provider_id_from_config(config_text: &str) -> Option<String> {
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let provider_id = active_codex_model_provider_id(&doc)?;

    if is_custom_codex_model_provider_id(&provider_id) {
        Some(provider_id)
    } else {
        None
    }
}

fn codex_model_provider_id_with_table_from_config(
    config_text: &str,
) -> Result<Option<String>, AppError> {
    if config_text.trim().is_empty() {
        return Ok(None);
    }

    let doc = config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?;
    let Some(provider_id) = active_codex_model_provider_id(&doc) else {
        return Ok(None);
    };

    let has_provider_table = doc
        .get("model_providers")
        .and_then(|item| item.as_table())
        .and_then(|table| table.get(provider_id.as_str()))
        .is_some();

    Ok(has_provider_table.then_some(provider_id))
}

fn primary_codex_model_provider_id_with_table(doc: &DocumentMut) -> Option<String> {
    if let Some(provider_id) = active_codex_model_provider_id(doc) {
        let has_provider_table = doc
            .get("model_providers")
            .and_then(|item| item.as_table_like())
            .and_then(|table| table.get(provider_id.as_str()))
            .is_some();
        if has_provider_table {
            return Some(provider_id);
        }
    }

    let providers = doc
        .get("model_providers")
        .and_then(|item| item.as_table_like())?;
    let mut keys = providers.iter().map(|(key, _)| key.to_string());
    let first = keys.next()?;
    keys.next().is_none().then_some(first)
}

fn normalize_codex_live_config_model_provider_with_anchors<'a>(
    config_text: &str,
    anchor_config_texts: impl IntoIterator<Item = &'a str>,
) -> Result<String, AppError> {
    if config_text.trim().is_empty() {
        return Ok(config_text.to_string());
    }

    let mut doc = config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?;

    let Some(source_provider_id) = active_codex_model_provider_id(&doc) else {
        return Ok(config_text.to_string());
    };

    let has_source_provider_table = doc
        .get("model_providers")
        .and_then(|item| item.as_table())
        .and_then(|table| table.get(source_provider_id.as_str()))
        .is_some();
    if !has_source_provider_table {
        return Ok(config_text.to_string());
    }

    let stable_provider_id = anchor_config_texts
        .into_iter()
        .find_map(stable_codex_model_provider_id_from_config)
        .or_else(|| {
            is_custom_codex_model_provider_id(&source_provider_id)
                .then(|| source_provider_id.clone())
        })
        .unwrap_or_else(|| CC_SWITCH_CODEX_MODEL_PROVIDER_ID.to_string());

    if stable_provider_id == source_provider_id {
        return Ok(config_text.to_string());
    }

    if let Some(model_providers) = doc
        .get_mut("model_providers")
        .and_then(|item| item.as_table_mut())
    {
        let Some(provider_table) = model_providers.remove(source_provider_id.as_str()) else {
            return Ok(config_text.to_string());
        };
        model_providers[stable_provider_id.as_str()] = provider_table;
    }

    rewrite_codex_profile_model_provider_refs(&mut doc, &source_provider_id, &stable_provider_id);
    doc["model_provider"] = toml_edit::value(stable_provider_id.as_str());

    Ok(doc.to_string())
}

fn rewrite_codex_profile_model_provider_refs(
    doc: &mut DocumentMut,
    source_provider_id: &str,
    stable_provider_id: &str,
) {
    let Some(profiles) = doc
        .get_mut("profiles")
        .and_then(|item| item.as_table_like_mut())
    else {
        return;
    };

    let profile_keys: Vec<String> = profiles.iter().map(|(key, _)| key.to_string()).collect();
    for profile_key in profile_keys {
        let Some(profile_table) = profiles
            .get_mut(&profile_key)
            .and_then(|item| item.as_table_like_mut())
        else {
            continue;
        };

        let references_source = profile_table
            .get("model_provider")
            .and_then(|item| item.as_str())
            == Some(source_provider_id);
        if references_source {
            profile_table.insert("model_provider", toml_edit::value(stable_provider_id));
        }
    }
}

/// Keep Codex's active `model_provider` stable across CC Switch provider changes.
///
/// Codex stores and filters resume history by `model_provider`, so switching between
/// provider-specific ids like `rightcode` and `aihubmix` makes history appear to move.
/// We preserve an existing custom provider id when possible and only rewrite the
/// live config text that Codex sees at provider-driven write boundaries.
pub fn normalize_codex_settings_config_model_provider(
    settings: &mut Value,
    anchor_config_text: Option<&str>,
) -> Result<(), AppError> {
    let Some(config_text) = settings
        .get("config")
        .and_then(|value| value.as_str())
        .map(str::to_string)
    else {
        return Ok(());
    };

    let current_config_text = read_codex_config_text().ok();
    let anchors = anchor_config_text
        .into_iter()
        .chain(current_config_text.as_deref());
    let normalized =
        normalize_codex_live_config_model_provider_with_anchors(&config_text, anchors)?;

    if let Some(obj) = settings.as_object_mut() {
        obj.insert("config".to_string(), Value::String(normalized));
    }

    Ok(())
}

fn restore_codex_backfill_model_provider_id(
    config_text: &str,
    template_config_text: &str,
) -> Result<String, AppError> {
    let Some(template_provider_id) =
        codex_model_provider_id_with_table_from_config(template_config_text)?
    else {
        return Ok(config_text.to_string());
    };

    if config_text.trim().is_empty() {
        return Ok(config_text.to_string());
    }

    let mut doc = config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?;
    let Some(live_provider_id) = active_codex_model_provider_id(&doc) else {
        return Ok(config_text.to_string());
    };

    if live_provider_id == template_provider_id {
        return Ok(config_text.to_string());
    }

    if let Some(model_providers) = doc
        .get_mut("model_providers")
        .and_then(|item| item.as_table_mut())
    {
        let Some(provider_table) = model_providers.remove(live_provider_id.as_str()) else {
            return Ok(config_text.to_string());
        };
        model_providers[template_provider_id.as_str()] = provider_table;
    } else {
        return Ok(config_text.to_string());
    }

    rewrite_codex_profile_model_provider_refs(&mut doc, &live_provider_id, &template_provider_id);
    doc["model_provider"] = toml_edit::value(template_provider_id.as_str());

    Ok(doc.to_string())
}

/// Convert a Codex live config that was normalized for history stability back
/// to the provider-specific id used by the stored provider template.
pub fn restore_codex_settings_config_model_provider_for_backfill(
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
    let Some(template_config_text) = template_settings
        .get("config")
        .and_then(|value| value.as_str())
    else {
        return Ok(());
    };

    let restored = restore_codex_backfill_model_provider_id(&config_text, template_config_text)?;
    if let Some(obj) = settings.as_object_mut() {
        obj.insert("config".to_string(), Value::String(restored));
    }

    Ok(())
}

/// Merge a stored provider snapshot into the current live config.
///
/// Strategy: edit the **live** document in place, overlaying only the entries the
/// snapshot explicitly provides. Everything else stays untouched — in particular
/// every comment the user has placed in `~/.codex/config.toml` survives a
/// provider switch, including:
///
/// - whole commented-out subtables (e.g. `# [mcp_servers.x] / # command = ...`,
///   typically used to temporarily disable an MCP server),
/// - commented-out root-level keys (e.g. `# disable_response_storage = true`),
/// - free-floating header notes attached to a key (toml_edit treats these as
///   the next key's prefix decor; overwriting that key in place preserves the
///   decor because the key already existed in live),
/// - trailing notes at end of file (document-level trailing decor).
///
/// Rules:
///
/// - `[mcp_servers]` is **never** overwritten from the snapshot. The user's live
///   `[mcp_servers]` content (active subtables, commented-out subtables, and any
///   loose comments around them) is preserved verbatim.
/// - Root-level user preference keys (`approval_mode`, `disable_response_storage`,
///   `model_reasoning_effort`, `check_for_update_on_startup`):
///     - `preserve_user_preferences = true` (provider switch): if live already has
///       the key, live wins and the snapshot's value is ignored; if live doesn't
///       have it, fall back to the snapshot's value. This way an initial write
///       still seeds preferences from the snapshot (which is what carries the
///       merged common-snippet defaults).
///     - `preserve_user_preferences = false` (common-snippet apply/clear):
///       snapshot drives preferences. Any preference key present in live but
///       absent from the snapshot is removed, so old snippet leftovers don't
///       bleed through.
/// - Every other top-level entry the snapshot supplies (root keys like
///   `model_provider` / `model`, tables like `[model_providers]`, `[projects]`,
///   `[features]`, …) overwrites the same entry in live. Replacing `[model_providers]`
///   wholesale is intentional — it ensures provider isolation, since the previous
///   provider's `[model_providers.X]` entries are not relevant under the new key.
pub fn merge_provider_into_codex_live_config(
    live_text: &str,
    provider_snapshot: &str,
    preserve_user_preferences: bool,
) -> Result<String, AppError> {
    const PREFERENCE_KEYS: &[&str] = &[
        "approval_mode",
        "disable_response_storage",
        "model_reasoning_effort",
        "check_for_update_on_startup",
    ];

    let mut live = if live_text.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        live_text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| AppError::Message(format!("Invalid Codex live config.toml: {e}")))?
    };

    let snap = if provider_snapshot.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        provider_snapshot
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| AppError::Message(format!("Invalid Codex provider snapshot: {e}")))?
    };

    // Step 1: drop any live entry the snapshot does not cover, so switching
    // providers doesn't leak the previous provider's tables (e.g. [projects],
    // [model_providers.OLD]) into the new live config. Exempt:
    //   - `[mcp_servers]` — always lives in the live config and is user-owned
    //     (including any comment-only lines around it).
    //   - preference keys, but only when `preserve_user_preferences` is true.
    let live_keys: Vec<String> = live.as_table().iter().map(|(k, _)| k.to_string()).collect();
    for key in live_keys {
        if key == "mcp_servers" {
            continue;
        }
        if preserve_user_preferences && PREFERENCE_KEYS.contains(&key.as_str()) {
            continue;
        }
        if snap.get(&key).is_some() {
            continue;
        }
        live.as_table_mut().remove(&key);
    }

    // Step 2: overlay every snapshot entry except [mcp_servers]. For preference
    // keys: when `preserve_user_preferences` is true and live already has the
    // key, live wins; otherwise the snapshot's value is used (this is what
    // seeds an initial write from snapshot, including merged common-snippet
    // defaults).
    let snap_keys: Vec<String> = snap.as_table().iter().map(|(k, _)| k.to_string()).collect();
    for key in snap_keys {
        if key == "mcp_servers" {
            continue;
        }
        if preserve_user_preferences
            && PREFERENCE_KEYS.contains(&key.as_str())
            && live.get(&key).is_some()
        {
            continue;
        }
        if let Some(val) = snap.get(&key) {
            live[key.as_str()] = val.clone();
        }
    }

    Ok(live.to_string())
}

/// Rewrite a stored Codex provider snapshot to use a specific provider key.
///
/// This updates both the root `model_provider` and the matching
/// `[model_providers.<key>]` table, while preserving the provider table body and
/// profile references.
pub fn rewrite_codex_config_model_provider_key(
    config_text: &str,
    target_provider_id: &str,
) -> Result<String, AppError> {
    if config_text.trim().is_empty() {
        return Ok(String::new());
    }

    let target_provider_id = clean_codex_provider_key(target_provider_id);
    let mut doc = config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?;
    let Some(source_provider_id) = primary_codex_model_provider_id_with_table(&doc) else {
        return Ok(config_text.to_string());
    };

    if let Some(model_providers) = doc
        .get_mut("model_providers")
        .and_then(|item| item.as_table_mut())
    {
        if source_provider_id != target_provider_id {
            let Some(provider_table) = model_providers.remove(source_provider_id.as_str()) else {
                return Ok(config_text.to_string());
            };
            model_providers[target_provider_id.as_str()] = provider_table;
            rewrite_codex_profile_model_provider_refs(
                &mut doc,
                &source_provider_id,
                &target_provider_id,
            );
        }
    } else {
        return Ok(config_text.to_string());
    }

    doc["model_provider"] = toml_edit::value(target_provider_id.as_str());

    Ok(doc.to_string())
}

/// Atomically write Codex live config after normalizing provider-specific ids.
///
/// Use this for provider-driven live writes. Keep `write_codex_live_atomic` available
/// for exact restore/backup paths that must preserve the config text semantically as saved.
pub fn write_codex_live_atomic_with_stable_provider(
    auth: &Value,
    config_text_opt: Option<&str>,
) -> Result<(), AppError> {
    write_codex_live_atomic_optional_auth_with_stable_provider(Some(auth), config_text_opt)
}

pub fn write_codex_live_atomic_optional_auth_with_stable_provider(
    auth: Option<&Value>,
    config_text_opt: Option<&str>,
) -> Result<(), AppError> {
    match config_text_opt {
        Some(config_text) => {
            let mut settings = serde_json::Map::new();
            settings.insert("config".to_string(), Value::String(config_text.to_string()));
            let mut settings = Value::Object(settings);
            normalize_codex_settings_config_model_provider(&mut settings, None)?;
            let config_text = settings
                .get("config")
                .and_then(|value| value.as_str())
                .unwrap_or(config_text);
            write_codex_live_atomic_optional_auth(auth, Some(config_text))
        }
        None => write_codex_live_atomic_optional_auth(auth, None),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_preserves_user_preferences_and_mcp_from_live() {
        // Current live config has user preferences and MCP servers
        let live = indoc::indoc! {r#"
            model_provider = "old-provider"
            model = "old-model"
            disable_response_storage = true
            model_reasoning_effort = "xhigh"
            approval_mode = "auto-edit"
            check_for_update_on_startup = false

            [model_providers.old-provider]
            name = "Old"

            [projects."/tmp/work"]
            trusted = true

            [mcp_servers.cargo-mcp]
            type = "stdio"
        "#};

        // Snapshot has different provider and its own [projects], no [mcp_servers]
        let snapshot = indoc::indoc! {r#"
            model_provider = "new-provider"
            model = "new-model"
            approval_mode = "suggest"

            [model_providers.new-provider]
            name = "New"
            api_key = "sk-test"

            [projects."/tmp/other"]
            trusted = true
        "#};

        let merged = merge_provider_into_codex_live_config(live, snapshot, true).unwrap();
        let doc: toml_edit::DocumentMut = merged.parse().unwrap();

        // Provider fields come from snapshot
        assert_eq!(doc["model_provider"].as_str(), Some("new-provider"));
        assert_eq!(doc["model"].as_str(), Some("new-model"));
        assert!(doc
            .get("model_providers")
            .unwrap()
            .get("new-provider")
            .is_some());
        assert!(doc
            .get("model_providers")
            .unwrap()
            .get("old-provider")
            .is_none());

        // User preferences come from live (not snapshot's approval_mode)
        assert_eq!(doc["disable_response_storage"].as_bool(), Some(true));
        assert_eq!(doc["model_reasoning_effort"].as_str(), Some("xhigh"));
        assert_eq!(doc["approval_mode"].as_str(), Some("auto-edit"));
        assert_eq!(doc["check_for_update_on_startup"].as_bool(), Some(false));

        // [projects] comes from snapshot (provider isolation)
        assert!(doc.get("projects").unwrap().get("/tmp/other").is_some());
        assert!(doc.get("projects").unwrap().get("/tmp/work").is_none());

        // [mcp_servers] comes from live (preserves comments and manual edits)
        assert!(doc.get("mcp_servers").is_some());
        assert!(doc.get("mcp_servers").unwrap().get("cargo-mcp").is_some());
    }

    #[test]
    fn merge_keeps_commented_out_mcp_entries_verbatim() {
        // The user has temporarily disabled an MCP server by commenting out
        // its whole subtable. Switching providers must NOT uncomment these
        // lines or drop them. Also exercises a comment-only line before an
        // active subtable, and a trailing comment after a value.
        let live = "\
model_provider = \"old\"
approval_mode = \"suggest\"

# top-level note for mcp_servers section
[mcp_servers.active]
# comment before command
command = \"runme\" # trailing comment

# this one is temporarily disabled
# [mcp_servers.disabled]
# command = \"nope\"
# args = [\"--off\"]
";

        let snapshot = "\
model_provider = \"new\"

[model_providers.new]
name = \"New\"
";

        let merged = merge_provider_into_codex_live_config(live, snapshot, true).unwrap();

        // Every comment line from the live mcp_servers region must survive verbatim.
        for needle in [
            "# top-level note for mcp_servers section",
            "# comment before command",
            "# trailing comment",
            "# this one is temporarily disabled",
            "# [mcp_servers.disabled]",
            "# command = \"nope\"",
            "# args = [\"--off\"]",
        ] {
            assert!(
                merged.contains(needle),
                "merged output is missing comment line: {needle:?}\n--- merged ---\n{merged}"
            );
        }

        // And the structural parts still parse and resolve as expected.
        let doc: toml_edit::DocumentMut = merged.parse().unwrap();
        assert_eq!(doc["model_provider"].as_str(), Some("new"));
        assert!(doc
            .get("mcp_servers")
            .and_then(|t| t.get("active"))
            .is_some());
        assert!(doc
            .get("mcp_servers")
            .and_then(|t| t.get("disabled"))
            .is_none());
    }

    #[test]
    fn merge_seeds_preferences_from_snapshot_when_live_is_empty() {
        // Initial write path: live config doesn't exist yet, snapshot carries
        // merged common-snippet defaults like disable_response_storage.
        // With preserve_user_preferences=true, the snapshot's preferences must
        // still land in the resulting file — otherwise the common snippet is lost.
        let snapshot = indoc::indoc! {r#"
            model_provider = "first"
            model = "gpt-5.2-codex"
            disable_response_storage = true
            approval_mode = "suggest"

            [model_providers.first]
            base_url = "https://api.example/v1"
        "#};

        let merged = merge_provider_into_codex_live_config("", snapshot, true).unwrap();
        assert!(
            merged.contains("disable_response_storage = true"),
            "snapshot-provided preference should seed an empty live config\n--- merged ---\n{merged}"
        );
        assert!(merged.contains("approval_mode = \"suggest\""));
        assert!(merged.contains("model_provider = \"first\""));
    }

    #[test]
    fn merge_with_preserve_false_drops_live_prefs_missing_from_snapshot() {
        // Common-snippet "clear" path: snapshot has no preference keys, so live
        // preferences left behind by a previous snippet must be removed.
        let live = indoc::indoc! {r#"
            model_provider = "p1"
            disable_response_storage = true
            model_reasoning_effort = "xhigh"

            [model_providers.p1]
            base_url = "https://a"
        "#};

        let snapshot = indoc::indoc! {r#"
            model_provider = "p1"

            [model_providers.p1]
            base_url = "https://a"
        "#};

        let merged = merge_provider_into_codex_live_config(live, snapshot, false).unwrap();
        let doc: toml_edit::DocumentMut = merged.parse().unwrap();
        assert!(doc.get("disable_response_storage").is_none());
        assert!(doc.get("model_reasoning_effort").is_none());
        assert_eq!(doc["model_provider"].as_str(), Some("p1"));
    }

    #[test]
    fn merge_keeps_root_level_comments_around_overwritten_keys() {
        // In toml_edit's model, comment-only lines between two root-level
        // keys are attached to the **next** key's prefix decor. When the
        // snapshot overwrites that next key, the comments must still be
        // there afterwards — otherwise users lose any inline notes they
        // sprinkled into ~/.codex/config.toml.
        let live = "\
# pinned by me — do not change without checking the runbook
model_provider = \"old\"

# disabled while debugging issue-1234
# disable_response_storage = true
approval_mode = \"auto-edit\"

# trailing footnote at EOF
";

        let snapshot = "\
model_provider = \"new\"

[model_providers.new]
name = \"New\"
";

        let merged = merge_provider_into_codex_live_config(live, snapshot, true).unwrap();

        for needle in [
            "# pinned by me — do not change without checking the runbook",
            "# disabled while debugging issue-1234",
            "# disable_response_storage = true",
            "# trailing footnote at EOF",
        ] {
            assert!(
                merged.contains(needle),
                "merged output is missing comment line: {needle:?}\n--- merged ---\n{merged}"
            );
        }

        let doc: toml_edit::DocumentMut = merged.parse().unwrap();
        assert_eq!(doc["model_provider"].as_str(), Some("new"));
        assert_eq!(doc["approval_mode"].as_str(), Some("auto-edit"));
    }

    #[test]
    fn merge_omits_mcp_section_when_live_has_none() {
        let live = indoc::indoc! {r#"
            model_provider = "foo"
            approval_mode = "suggest"
        "#};

        let snapshot = indoc::indoc! {r#"
            model_provider = "bar"
        "#};

        let merged = merge_provider_into_codex_live_config(live, snapshot, true).unwrap();
        let doc: toml_edit::DocumentMut = merged.parse().unwrap();

        assert_eq!(doc["model_provider"].as_str(), Some("bar"));
        assert!(doc.get("mcp_servers").is_none());
        assert_eq!(doc["approval_mode"].as_str(), Some("suggest"));
    }
    use crate::app_config::AppType;
    use crate::test_support::{lock_test_home_and_settings, set_test_home_override};
    use std::ffi::OsString;
    use std::path::Path;

    struct CodexHomeEnvGuard {
        original: Option<OsString>,
    }

    impl CodexHomeEnvGuard {
        fn new(value: Option<&str>) -> Self {
            let original = std::env::var_os("CODEX_HOME");
            match value {
                Some(v) => unsafe { std::env::set_var("CODEX_HOME", v) },
                None => unsafe { std::env::remove_var("CODEX_HOME") },
            }
            Self { original }
        }
    }

    impl Drop for CodexHomeEnvGuard {
        fn drop(&mut self) {
            match self.original.as_ref() {
                Some(value) => unsafe { std::env::set_var("CODEX_HOME", value) },
                None => unsafe { std::env::remove_var("CODEX_HOME") },
            }
        }
    }

    struct SettingsGuard {
        original: crate::settings::AppSettings,
    }

    impl SettingsGuard {
        fn with_codex_config_dir(dir: Option<&str>) -> Self {
            let original = crate::settings::get_settings();
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
    fn get_codex_config_dir_respects_codex_home_env_var_and_tilde() {
        let _guard = lock_test_home_and_settings();
        let _settings = SettingsGuard::with_codex_config_dir(None);
        let _env = CodexHomeEnvGuard::new(Some("~/.config/codex"));
        set_test_home_override(Some(Path::new("/tmp/codex-home-tilde")));

        assert_eq!(
            get_codex_config_dir(),
            PathBuf::from("/tmp/codex-home-tilde")
                .join(".config")
                .join("codex")
        );

        set_test_home_override(None);
    }

    #[test]
    fn get_codex_config_dir_env_overrides_settings_override() {
        let _guard = lock_test_home_and_settings();
        let _settings = SettingsGuard::with_codex_config_dir(Some("/tmp/settings-codex"));
        let _env = CodexHomeEnvGuard::new(Some("/tmp/env-codex"));
        set_test_home_override(Some(Path::new("/tmp/codex-home")));

        assert_eq!(get_codex_config_dir(), PathBuf::from("/tmp/env-codex"));

        set_test_home_override(None);
    }

    #[test]
    fn codex_live_sync_detects_initialized_codex_home_from_env() {
        let _guard = lock_test_home_and_settings();
        let _settings = SettingsGuard::with_codex_config_dir(None);
        let env_home = PathBuf::from("/tmp/codex-live-sync-env");
        let _env = CodexHomeEnvGuard::new(Some(env_home.to_str().unwrap()));
        set_test_home_override(Some(Path::new("/tmp/codex-live-sync-home")));

        std::fs::create_dir_all(&env_home).expect("create CODEX_HOME");

        assert!(crate::sync_policy::should_sync_live(&AppType::Codex));

        let _ = std::fs::remove_dir_all(&env_home);
        set_test_home_override(None);
    }

    #[test]
    fn normalize_live_config_preserves_current_custom_model_provider_id() {
        let current = r#"model_provider = "rightcode"

[model_providers.rightcode]
name = "RightCode"
base_url = "https://rightcode.example/v1"
wire_api = "responses"
"#;
        let target = r#"model_provider = "aihubmix"
model = "gpt-5.4"

[model_providers.aihubmix]
name = "AiHubMix"
base_url = "https://aihubmix.example/v1"
wire_api = "responses"
requires_openai_auth = true

[mcp_servers.context7]
command = "npx"
"#;

        let result =
            normalize_codex_live_config_model_provider_with_anchors(target, Some(current)).unwrap();
        let parsed: toml::Value = toml::from_str(&result).unwrap();

        assert_eq!(
            parsed.get("model_provider").and_then(|v| v.as_str()),
            Some("rightcode")
        );

        let model_providers = parsed
            .get("model_providers")
            .and_then(|v| v.as_table())
            .expect("model_providers should exist");
        assert!(
            model_providers.get("aihubmix").is_none(),
            "source provider id should not remain in live config"
        );

        let stable_provider = model_providers
            .get("rightcode")
            .expect("stable provider table should exist");
        assert_eq!(
            stable_provider.get("base_url").and_then(|v| v.as_str()),
            Some("https://aihubmix.example/v1")
        );
        assert!(
            parsed.get("mcp_servers").is_some(),
            "unrelated config should be preserved"
        );
    }

    #[test]
    fn normalize_live_config_uses_target_custom_provider_when_current_is_reserved() {
        let current = r#"model_provider = "openai""#;
        let target = r#"model_provider = "aihubmix"

[model_providers.aihubmix]
name = "AiHubMix"
base_url = "https://aihubmix.example/v1"
wire_api = "responses"
"#;

        let result =
            normalize_codex_live_config_model_provider_with_anchors(target, Some(current)).unwrap();
        let parsed: toml::Value = toml::from_str(&result).unwrap();

        assert_eq!(
            parsed.get("model_provider").and_then(|v| v.as_str()),
            Some("aihubmix")
        );
        assert!(
            parsed
                .get("model_providers")
                .and_then(|v| v.get("aihubmix"))
                .is_some(),
            "target provider id should be kept when there is no reusable live custom id"
        );
    }

    #[test]
    fn normalize_live_config_leaves_official_empty_config_unchanged() {
        let current = r#"model_provider = "rightcode"

[model_providers.rightcode]
base_url = "https://rightcode.example/v1"
"#;

        let result =
            normalize_codex_live_config_model_provider_with_anchors("", Some(current)).unwrap();

        assert_eq!(result, "");
    }

    #[test]
    fn normalize_live_config_rewrites_matching_profile_model_provider_refs() {
        let current = r#"model_provider = "session_anchor"

[model_providers.session_anchor]
name = "Session Anchor"
base_url = "https://anchor.example/v1"
wire_api = "responses"
"#;
        let target = r#"model_provider = "vendor_alpha"
model = "gpt-5.4"
profile = "work"

[model_providers.vendor_alpha]
name = "Vendor Alpha"
base_url = "https://alpha.example/v1"
wire_api = "responses"

[profiles.work]
model_provider = "vendor_alpha"
model = "gpt-5.4"
"#;

        let result =
            normalize_codex_live_config_model_provider_with_anchors(target, Some(current)).unwrap();
        let parsed: toml::Value = toml::from_str(&result).unwrap();

        assert_eq!(
            parsed.get("model_provider").and_then(|v| v.as_str()),
            Some("session_anchor")
        );
        assert_eq!(
            parsed
                .get("profiles")
                .and_then(|v| v.get("work"))
                .and_then(|v| v.get("model_provider"))
                .and_then(|v| v.as_str()),
            Some("session_anchor"),
            "profile override matching the rewritten provider should stay valid"
        );
    }

    #[test]
    fn normalize_live_config_keeps_unrelated_profile_model_provider_refs() {
        let current = r#"model_provider = "session_anchor"

[model_providers.session_anchor]
name = "Session Anchor"
base_url = "https://anchor.example/v1"
wire_api = "responses"
"#;
        let target = r#"model_provider = "vendor_alpha"
model = "gpt-5.4"

[model_providers.vendor_alpha]
name = "Vendor Alpha"
base_url = "https://alpha.example/v1"
wire_api = "responses"

[model_providers.local_profile]
name = "Local Profile"
base_url = "http://localhost:11434/v1"
wire_api = "responses"

[profiles.local]
model_provider = "local_profile"
model = "local-model"
"#;

        let result =
            normalize_codex_live_config_model_provider_with_anchors(target, Some(current)).unwrap();
        let parsed: toml::Value = toml::from_str(&result).unwrap();

        assert_eq!(
            parsed
                .get("profiles")
                .and_then(|v| v.get("local"))
                .and_then(|v| v.get("model_provider"))
                .and_then(|v| v.as_str()),
            Some("local_profile"),
            "unrelated profile provider references should be preserved"
        );
        assert!(
            parsed
                .get("model_providers")
                .and_then(|v| v.get("local_profile"))
                .is_some(),
            "unrelated provider tables should also remain available"
        );
    }

    #[test]
    fn normalize_live_config_keeps_stable_provider_across_repeated_switches() {
        let anchor = r#"model_provider = "session_anchor"

[model_providers.session_anchor]
name = "Session Anchor"
base_url = "https://anchor.example/v1"
wire_api = "responses"
"#;
        let first_target = r#"model_provider = "vendor_alpha"

[model_providers.vendor_alpha]
name = "Vendor Alpha"
base_url = "https://alpha.example/v1"
wire_api = "responses"
"#;
        let second_target = r#"model_provider = "vendor_beta"

[model_providers.vendor_beta]
name = "Vendor Beta"
base_url = "https://beta.example/v1"
wire_api = "responses"
"#;

        let first =
            normalize_codex_live_config_model_provider_with_anchors(first_target, Some(anchor))
                .unwrap();
        let second = normalize_codex_live_config_model_provider_with_anchors(
            second_target,
            Some(first.as_str()),
        )
        .unwrap();
        let parsed: toml::Value = toml::from_str(&second).unwrap();

        assert_eq!(
            parsed.get("model_provider").and_then(|v| v.as_str()),
            Some("session_anchor"),
            "stable provider id should not drift across repeated switches"
        );
        assert_eq!(
            parsed
                .get("model_providers")
                .and_then(|v| v.get("session_anchor"))
                .and_then(|v| v.get("base_url"))
                .and_then(|v| v.as_str()),
            Some("https://beta.example/v1")
        );
    }

    #[test]
    fn restore_backfill_config_rewrites_live_id_to_template_provider_id() {
        let mut settings = serde_json::json!({
            "config": r#"model_provider = "session_anchor"
model = "gpt-5.4"
profile = "work"

[model_providers.session_anchor]
name = "AiHubMix"
base_url = "https://aihubmix.example/v1"
wire_api = "responses"

[profiles.work]
model_provider = "session_anchor"
model = "gpt-5.4"
"#
        });
        let template = serde_json::json!({
            "config": r#"model_provider = "aihubmix"

[model_providers.aihubmix]
name = "AiHubMix"
base_url = "https://aihubmix.example/v1"
"#
        });

        restore_codex_settings_config_model_provider_for_backfill(&mut settings, &template)
            .unwrap();

        let config = settings.get("config").and_then(Value::as_str).unwrap();
        let parsed: toml::Value = toml::from_str(config).unwrap();
        assert_eq!(
            parsed.get("model_provider").and_then(|v| v.as_str()),
            Some("aihubmix")
        );
        assert!(parsed
            .get("model_providers")
            .and_then(|v| v.get("aihubmix"))
            .is_some());
        assert_eq!(
            parsed
                .get("profiles")
                .and_then(|v| v.get("work"))
                .and_then(|v| v.get("model_provider"))
                .and_then(|v| v.as_str()),
            Some("aihubmix")
        );
    }
}
