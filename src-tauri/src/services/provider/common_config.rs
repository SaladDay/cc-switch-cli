use serde_json::Value;
use toml_edit::{DocumentMut, Item, TableLike};

use crate::app_config::{AppType, MultiAppConfig};
use crate::database::Database;
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta};

use super::ProviderService;

const MIGRATION_MARKER: &str = "common_config_upstream_semantics_migrated_v1";
const CODEX_RUNTIME_KEYS: &[&str] = &["projects", "trusted_workspaces"];
const CODEX_IDENTITY_KEYS: &[&str] = &["model", "model_provider", "model_providers"];

fn json_is_subset(target: &Value, source: &Value) -> bool {
    match source {
        Value::Object(source_map) => {
            let Some(target_map) = target.as_object() else {
                return false;
            };
            source_map.iter().all(|(key, source_value)| {
                target_map
                    .get(key)
                    .is_some_and(|target_value| json_is_subset(target_value, source_value))
            })
        }
        Value::Array(source_arr) => {
            let Some(target_arr) = target.as_array() else {
                return false;
            };
            json_array_contains_subset(target_arr, source_arr)
        }
        _ => target == source,
    }
}

fn json_array_contains_subset(target_arr: &[Value], source_arr: &[Value]) -> bool {
    let mut matched = vec![false; target_arr.len()];

    source_arr.iter().all(|source_item| {
        if let Some((index, _)) = target_arr.iter().enumerate().find(|(index, target_item)| {
            !matched[*index] && json_is_subset(target_item, source_item)
        }) {
            matched[index] = true;
            true
        } else {
            false
        }
    })
}

fn json_remove_array_items(target_arr: &mut Vec<Value>, source_arr: &[Value]) {
    for source_item in source_arr {
        if let Some(index) = target_arr
            .iter()
            .position(|target_item| json_is_subset(target_item, source_item))
        {
            target_arr.remove(index);
        }
    }
}

fn json_deep_merge(target: &mut Value, source: &Value) {
    match (target, source) {
        (Value::Object(target_map), Value::Object(source_map)) => {
            for (key, source_value) in source_map {
                match target_map.get_mut(key) {
                    Some(target_value) => json_deep_merge(target_value, source_value),
                    None => {
                        target_map.insert(key.clone(), source_value.clone());
                    }
                }
            }
        }
        (target_value, source_value) => {
            *target_value = source_value.clone();
        }
    }
}

fn json_deep_remove(target: &mut Value, source: &Value) {
    let (Some(target_map), Some(source_map)) = (target.as_object_mut(), source.as_object()) else {
        return;
    };

    for (key, source_value) in source_map {
        let mut remove_key = false;

        if let Some(target_value) = target_map.get_mut(key) {
            if source_value.is_object() && target_value.is_object() {
                json_deep_remove(target_value, source_value);
                remove_key = target_value.as_object().is_some_and(|obj| obj.is_empty());
            } else if let (Some(target_arr), Some(source_arr)) =
                (target_value.as_array_mut(), source_value.as_array())
            {
                json_remove_array_items(target_arr, source_arr);
                remove_key = target_arr.is_empty();
            } else if json_is_subset(target_value, source_value) {
                remove_key = true;
            }
        }

        if remove_key {
            target_map.remove(key);
        }
    }
}

fn toml_value_is_subset(target: &toml_edit::Value, source: &toml_edit::Value) -> bool {
    match (target, source) {
        (toml_edit::Value::String(target), toml_edit::Value::String(source)) => {
            target.value() == source.value()
        }
        (toml_edit::Value::Integer(target), toml_edit::Value::Integer(source)) => {
            target.value() == source.value()
        }
        (toml_edit::Value::Float(target), toml_edit::Value::Float(source)) => {
            target.value() == source.value()
        }
        (toml_edit::Value::Boolean(target), toml_edit::Value::Boolean(source)) => {
            target.value() == source.value()
        }
        (toml_edit::Value::Datetime(target), toml_edit::Value::Datetime(source)) => {
            target.value() == source.value()
        }
        (toml_edit::Value::Array(target), toml_edit::Value::Array(source)) => {
            toml_array_contains_subset(target, source)
        }
        (toml_edit::Value::InlineTable(target), toml_edit::Value::InlineTable(source)) => {
            source.iter().all(|(key, source_item)| {
                target
                    .get(key)
                    .is_some_and(|target_item| toml_value_is_subset(target_item, source_item))
            })
        }
        _ => false,
    }
}

fn toml_array_contains_subset(target: &toml_edit::Array, source: &toml_edit::Array) -> bool {
    let mut matched = vec![false; target.len()];
    let target_items: Vec<&toml_edit::Value> = target.iter().collect();

    source.iter().all(|source_item| {
        if let Some((index, _)) = target_items
            .iter()
            .enumerate()
            .find(|(index, target_item)| {
                !matched[*index] && toml_value_is_subset(target_item, source_item)
            })
        {
            matched[index] = true;
            true
        } else {
            false
        }
    })
}

fn toml_remove_array_items(target: &mut toml_edit::Array, source: &toml_edit::Array) {
    for source_item in source.iter() {
        let index = {
            let target_items: Vec<&toml_edit::Value> = target.iter().collect();
            target_items
                .iter()
                .enumerate()
                .find(|(_, target_item)| toml_value_is_subset(target_item, source_item))
                .map(|(index, _)| index)
        };

        if let Some(index) = index {
            target.remove(index);
        }
    }
}

fn toml_item_is_subset(target: &Item, source: &Item) -> bool {
    if let Some(source_table) = source.as_table_like() {
        let Some(target_table) = target.as_table_like() else {
            return false;
        };
        return source_table.iter().all(|(key, source_item)| {
            target_table
                .get(key)
                .is_some_and(|target_item| toml_item_is_subset(target_item, source_item))
        });
    }

    match (target.as_value(), source.as_value()) {
        (Some(target_value), Some(source_value)) => {
            toml_value_is_subset(target_value, source_value)
        }
        _ => false,
    }
}

fn merge_toml_item(target: &mut Item, source: &Item) {
    if let Some(source_table) = source.as_table_like() {
        if let Some(target_table) = target.as_table_like_mut() {
            merge_toml_table_like(target_table, source_table);
            return;
        }
    }

    *target = source.clone();
}

fn merge_toml_table_like(target: &mut dyn TableLike, source: &dyn TableLike) {
    for (key, source_item) in source.iter() {
        match target.get_mut(key) {
            Some(target_item) => merge_toml_item(target_item, source_item),
            None => {
                target.insert(key, source_item.clone());
            }
        }
    }
}

fn remove_toml_item(target: &mut Item, source: &Item) {
    if let Some(source_table) = source.as_table_like() {
        if let Some(target_table) = target.as_table_like_mut() {
            remove_toml_table_like(target_table, source_table);
            if target_table.is_empty() {
                *target = Item::None;
            }
            return;
        }
    }

    if let Some(source_value) = source.as_value() {
        let mut remove_item = false;

        if let Some(target_value) = target.as_value_mut() {
            match (target_value, source_value) {
                (toml_edit::Value::Array(target_arr), toml_edit::Value::Array(source_arr)) => {
                    toml_remove_array_items(target_arr, source_arr);
                    remove_item = target_arr.is_empty();
                }
                (target_value, source_value)
                    if toml_value_is_subset(target_value, source_value) =>
                {
                    remove_item = true;
                }
                _ => {}
            }
        }

        if remove_item {
            *target = Item::None;
        }
    }
}

fn remove_toml_table_like(target: &mut dyn TableLike, source: &dyn TableLike) {
    let keys: Vec<String> = source.iter().map(|(key, _)| key.to_string()).collect();

    for key in keys {
        let mut remove_key = false;
        if let (Some(target_item), Some(source_item)) = (target.get_mut(&key), source.get(&key)) {
            remove_toml_item(target_item, source_item);
            remove_key = target_item.is_none()
                || target_item
                    .as_table_like()
                    .is_some_and(|table_like| table_like.is_empty());
        }

        if remove_key {
            target.remove(&key);
        }
    }
}

fn parse_json_object_snippet(
    app_type: &AppType,
    snippet: &str,
    for_strip: bool,
) -> Result<Value, AppError> {
    let value: Value = serde_json::from_str(snippet).map_err(|e| match app_type {
        AppType::Claude => AppError::localized(
            "common_config.claude.invalid_json",
            format!("Claude 通用配置片段不是有效的 JSON：{e}"),
            format!("Claude common config snippet is not valid JSON: {e}"),
        ),
        AppType::Gemini => AppError::localized(
            "common_config.gemini.invalid_json",
            format!("Gemini 通用配置片段不是有效的 JSON：{e}"),
            format!("Gemini common config snippet is not valid JSON: {e}"),
        ),
        AppType::OpenCode | AppType::OpenClaw => AppError::localized(
            "common_config.opencode.invalid_json",
            format!("OpenCode 通用配置片段不是有效的 JSON：{e}"),
            format!("OpenCode common config snippet is not valid JSON: {e}"),
        ),
        AppType::Codex => AppError::Config(format!("Unexpected JSON common config parse: {e}")),
    })?;

    if !value.is_object() {
        return Err(match app_type {
            AppType::Claude => AppError::localized(
                "common_config.claude.not_object",
                "Claude 通用配置片段必须是 JSON 对象",
                "Claude common config snippet must be a JSON object",
            ),
            AppType::Gemini => AppError::localized(
                "common_config.gemini.not_object",
                "Gemini 通用配置片段必须是 JSON 对象",
                "Gemini common config snippet must be a JSON object",
            ),
            AppType::OpenCode | AppType::OpenClaw => AppError::localized(
                "common_config.opencode.not_object",
                "OpenCode 通用配置片段必须是 JSON 对象",
                "OpenCode common config snippet must be a JSON object",
            ),
            AppType::Codex => AppError::Config("Unexpected JSON common config type".into()),
        });
    }

    let mut value = value;
    if for_strip && matches!(app_type, AppType::Claude) {
        let _ = ProviderService::normalize_claude_models_in_value(&mut value);
    }

    Ok(value)
}

fn parse_codex_snippet(snippet: &str) -> Result<DocumentMut, AppError> {
    snippet
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Config(format!("Common config TOML parse error: {e}")))
}

fn remove_codex_keys(doc: &mut DocumentMut, keys: &[&str]) -> bool {
    let mut removed = false;
    for key in keys {
        removed |= doc.as_table_mut().remove(key).is_some();
    }
    removed
}

fn codex_sanitized_snippet_for_operation(
    snippet: &str,
    remove_identity_keys: bool,
) -> Result<Option<String>, AppError> {
    let mut doc = parse_codex_snippet(snippet)?;
    let removed_runtime = remove_codex_keys(&mut doc, CODEX_RUNTIME_KEYS);
    if remove_identity_keys {
        let _ = remove_codex_keys(&mut doc, CODEX_IDENTITY_KEYS);
    }
    if removed_runtime {
        log::warn!(
            "Ignoring Codex runtime-local keys in historical common config snippet: {}",
            CODEX_RUNTIME_KEYS.join(", ")
        );
    }
    if doc.as_table().is_empty() {
        return Ok(None);
    }
    Ok(Some(doc.to_string()))
}

fn codex_snippet_for_apply(snippet: &str) -> Result<Option<String>, AppError> {
    codex_sanitized_snippet_for_operation(snippet, false)
}

fn codex_snippet_for_remove(snippet: &str) -> Result<Option<String>, AppError> {
    codex_sanitized_snippet_for_operation(snippet, true)
}

pub(super) fn validate_common_config_snippet(
    app_type: &AppType,
    snippet: Option<&str>,
) -> Result<(), AppError> {
    let Some(snippet) = snippet.map(str::trim) else {
        return Ok(());
    };
    if snippet.is_empty() {
        return Ok(());
    }

    match app_type {
        AppType::Claude | AppType::Gemini | AppType::OpenCode | AppType::OpenClaw => {
            parse_json_object_snippet(app_type, snippet, false)?;
        }
        AppType::Codex => {
            let doc = parse_codex_snippet(snippet)?;
            if let Some(key) = CODEX_RUNTIME_KEYS
                .iter()
                .find(|key| doc.as_table().contains_key(**key))
            {
                return Err(AppError::localized(
                    "common_config.codex.runtime_key",
                    format!(
                        "Codex 通用配置不能包含运行时本地配置 `{key}`，请从片段中移除该表"
                    ),
                    format!(
                        "Codex common config must not contain runtime-local key `{key}`; remove it from the snippet"
                    ),
                ));
            }
        }
    }

    Ok(())
}

pub(super) fn settings_contain_common_config(
    app_type: &AppType,
    settings: &Value,
    snippet: &str,
) -> bool {
    let trimmed = snippet.trim();
    if trimmed.is_empty() {
        return false;
    }

    match app_type {
        AppType::Claude => match parse_json_object_snippet(app_type, trimmed, true) {
            Ok(source) => json_is_subset(settings, &source),
            Err(_) => false,
        },
        AppType::Codex => {
            let config_toml = settings.get("config").and_then(Value::as_str).unwrap_or("");
            if config_toml.trim().is_empty() {
                return false;
            }

            let target_doc = match config_toml.parse::<DocumentMut>() {
                Ok(doc) => doc,
                Err(_) => return false,
            };
            let source_snippet = match codex_snippet_for_apply(trimmed) {
                Ok(Some(snippet)) => snippet,
                Ok(None) | Err(_) => return false,
            };
            let source_doc = match source_snippet.parse::<DocumentMut>() {
                Ok(doc) => doc,
                Err(_) => return false,
            };

            toml_item_is_subset(target_doc.as_item(), source_doc.as_item())
        }
        AppType::Gemini => match parse_json_object_snippet(app_type, trimmed, false) {
            Ok(Value::Object(source_map)) => {
                let Some(target_map) = settings.get("env").and_then(Value::as_object) else {
                    return false;
                };
                source_map.iter().all(|(key, source_value)| {
                    target_map
                        .get(key)
                        .is_some_and(|target_value| json_is_subset(target_value, source_value))
                })
            }
            _ => false,
        },
        AppType::OpenCode | AppType::OpenClaw => false,
    }
}

pub(super) fn provider_uses_common_config(
    app_type: &AppType,
    provider: &Provider,
    snippet: Option<&str>,
) -> bool {
    match provider
        .meta
        .as_ref()
        .and_then(|meta| meta.apply_common_config)
    {
        Some(explicit) => explicit && snippet.is_some_and(|value| !value.trim().is_empty()),
        None => snippet.is_some_and(|value| {
            settings_contain_common_config(app_type, &provider.settings_config, value)
        }),
    }
}

pub(super) fn apply_common_config_to_settings(
    app_type: &AppType,
    settings: &Value,
    snippet: &str,
) -> Result<Value, AppError> {
    let trimmed = snippet.trim();
    if trimmed.is_empty() {
        return Ok(settings.clone());
    }

    match app_type {
        AppType::Claude => {
            let source = parse_json_object_snippet(app_type, trimmed, false)?;
            let mut result = settings.clone();
            json_deep_merge(&mut result, &source);
            Ok(result)
        }
        AppType::Codex => {
            let Some(snippet) = codex_snippet_for_apply(trimmed)? else {
                return Ok(settings.clone());
            };

            let mut result = settings.clone();
            let config_toml = settings.get("config").and_then(Value::as_str).unwrap_or("");
            let mut target_doc = if config_toml.trim().is_empty() {
                DocumentMut::new()
            } else {
                config_toml.parse::<DocumentMut>().map_err(|e| {
                    AppError::Config(format!(
                        "Invalid Codex config.toml while applying common config: {e}"
                    ))
                })?
            };
            let source_doc = parse_codex_snippet(&snippet)?;

            merge_toml_table_like(target_doc.as_table_mut(), source_doc.as_table());
            if let Some(obj) = result.as_object_mut() {
                obj.insert("config".to_string(), Value::String(target_doc.to_string()));
            }
            Ok(result)
        }
        AppType::Gemini => {
            let source = parse_json_object_snippet(app_type, trimmed, false)?;
            let mut result = settings.clone();
            if let Some(env) = result.get_mut("env") {
                json_deep_merge(env, &source);
            } else if let Some(obj) = result.as_object_mut() {
                obj.insert("env".to_string(), source);
            }
            Ok(result)
        }
        AppType::OpenCode | AppType::OpenClaw => Ok(settings.clone()),
    }
}

pub(super) fn remove_common_config_from_settings(
    app_type: &AppType,
    settings: &Value,
    snippet: &str,
) -> Result<Value, AppError> {
    let trimmed = snippet.trim();
    if trimmed.is_empty() {
        return Ok(settings.clone());
    }

    match app_type {
        AppType::Claude => {
            let source = parse_json_object_snippet(app_type, trimmed, true)?;
            let mut result = settings.clone();
            json_deep_remove(&mut result, &source);
            Ok(result)
        }
        AppType::Codex => {
            let Some(snippet) = codex_snippet_for_remove(trimmed)? else {
                return Ok(settings.clone());
            };

            let mut result = settings.clone();
            let config_toml = settings.get("config").and_then(Value::as_str).unwrap_or("");
            let mut target_doc = if config_toml.trim().is_empty() {
                DocumentMut::new()
            } else {
                config_toml.parse::<DocumentMut>().map_err(|e| {
                    AppError::Config(format!(
                        "Invalid Codex config.toml while removing common config: {e}"
                    ))
                })?
            };
            let source_doc = parse_codex_snippet(&snippet)?;

            remove_toml_table_like(target_doc.as_table_mut(), source_doc.as_table());
            if let Some(obj) = result.as_object_mut() {
                obj.insert("config".to_string(), Value::String(target_doc.to_string()));
            }
            Ok(result)
        }
        AppType::Gemini => {
            let source = parse_json_object_snippet(app_type, trimmed, false)?;
            let mut result = settings.clone();
            if let Some(env) = result.get_mut("env") {
                json_deep_remove(env, &source);
            }
            Ok(result)
        }
        AppType::OpenCode | AppType::OpenClaw => Ok(settings.clone()),
    }
}

pub(super) fn build_effective_settings_with_common_config(
    app_type: &AppType,
    provider: &Provider,
    snippet: Option<&str>,
    requested_apply_common_config: bool,
) -> Result<Value, AppError> {
    let mut effective_settings = provider.settings_config.clone();

    if requested_apply_common_config && provider_uses_common_config(app_type, provider, snippet) {
        if let Some(snippet_text) = snippet {
            match apply_common_config_to_settings(app_type, &effective_settings, snippet_text) {
                Ok(settings) => effective_settings = settings,
                Err(err) => {
                    log::warn!(
                        "Failed to apply common config for {} provider '{}': {err}",
                        app_type.as_str(),
                        provider.id
                    );
                }
            }
        }
    }

    Ok(effective_settings)
}

pub(super) fn strip_common_config_from_live_settings(
    app_type: &AppType,
    provider: &Provider,
    live_settings: Value,
    snippet: Option<&str>,
) -> Value {
    if !provider_uses_common_config(app_type, provider, snippet) {
        return live_settings;
    }

    let Some(snippet_text) = snippet else {
        return live_settings;
    };

    match remove_common_config_from_settings(app_type, &live_settings, snippet_text) {
        Ok(settings) => settings,
        Err(err) => {
            log::warn!(
                "Failed to strip common config for {} provider '{}': {err}",
                app_type.as_str(),
                provider.id
            );
            live_settings
        }
    }
}

pub(super) fn normalize_provider_common_config_for_storage(
    app_type: &AppType,
    provider: &mut Provider,
    snippet: Option<&str>,
) -> Result<(), AppError> {
    let uses_common_config = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.apply_common_config)
        .unwrap_or(false);

    if !uses_common_config {
        return Ok(());
    }

    let Some(snippet) = snippet.map(str::trim) else {
        return Ok(());
    };
    if snippet.is_empty() {
        return Ok(());
    }

    match remove_common_config_from_settings(app_type, &provider.settings_config, snippet) {
        Ok(settings) => provider.settings_config = settings,
        Err(err) => {
            log::warn!(
                "Failed to normalize common config before saving {} provider '{}': {err}",
                app_type.as_str(),
                provider.id
            );
        }
    }

    Ok(())
}

pub(super) fn migrate_provider_subset_usage_for_storage(
    app_type: &AppType,
    provider: &mut Provider,
    snippet: Option<&str>,
) -> Result<(), AppError> {
    if provider
        .meta
        .as_ref()
        .and_then(|meta| meta.apply_common_config)
        .is_none()
        && snippet.is_some_and(|value| {
            settings_contain_common_config(app_type, &provider.settings_config, value)
        })
    {
        provider
            .meta
            .get_or_insert_with(ProviderMeta::default)
            .apply_common_config = Some(true);
    }

    normalize_provider_common_config_for_storage(app_type, provider, snippet)
}

pub(crate) fn migrate_common_config_upstream_semantics_if_needed(
    db: &Database,
    config: &mut MultiAppConfig,
) -> Result<(), AppError> {
    if db
        .get_setting(MIGRATION_MARKER)?
        .as_deref()
        .is_some_and(|value| value == "true")
    {
        return Ok(());
    }

    for app_type in [AppType::Claude, AppType::Codex, AppType::Gemini] {
        let snippet = config
            .common_config_snippets
            .get(&app_type)
            .cloned()
            .filter(|value| !value.trim().is_empty());
        let Some(snippet) = snippet else {
            continue;
        };

        let Some(manager) = config.get_manager_mut(&app_type) else {
            continue;
        };

        for provider in manager.providers.values_mut() {
            if provider
                .meta
                .as_ref()
                .and_then(|meta| meta.apply_common_config)
                .is_none()
            {
                provider
                    .meta
                    .get_or_insert_with(ProviderMeta::default)
                    .apply_common_config = Some(true);
            }

            normalize_provider_common_config_for_storage(&app_type, provider, Some(&snippet))?;
            db.save_provider(app_type.as_str(), provider)?;
        }
    }

    db.set_setting(MIGRATION_MARKER, "true")?;
    Ok(())
}

#[cfg(test)]
pub(super) mod test_support {
    use super::*;

    pub(crate) fn remove(
        app_type: &AppType,
        settings: &Value,
        snippet: &str,
    ) -> Result<Value, AppError> {
        remove_common_config_from_settings(app_type, settings, snippet)
    }
}
