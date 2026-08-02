use serde_json::{json, Map, Value};
use std::collections::HashMap;
use toml_edit::{DocumentMut, Item, TableLike};

use crate::app_config::{AppType, MultiAppConfig};
use crate::claude_model_config::{
    claude_provider_owned_env_keys, CLAUDE_CONTEXT_WINDOW_ENV_KEYS, CLAUDE_ROUTED_MODEL_ENV_KEYS,
};
use crate::database::Database;
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta};
use crate::provider_preset_models::CODEX_OAUTH_CONTEXT_TOKENS;

const MIGRATION_MARKER: &str = "common_config_upstream_semantics_migrated_v1";
const GEMINI_CREDENTIAL_SCRUB_MARKER: &str = "gemini_common_config_credentials_scrubbed_v1";
const GEMINI_CREDENTIAL_SCRUB_AUDIT_KEY: &str = "gemini_common_config_scrub_audit_v1";

const CLAUDE_PROVIDER_OWNED_TOP_LEVEL_KEYS: &[&str] =
    &["apiBaseUrl", "primaryModel", "smallFastModel"];

/// Treat credential-shaped names as sensitive by default. Missing a new
/// provider's secret would spread it through every provider using the common
/// snippet; stripping one ordinary setting is the safer failure mode.
pub(super) fn is_sensitive_config_key(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();

    const SENSITIVE_SUFFIXES: &[&str] = &[
        "_KEY",
        "_API_KEY",
        "_ACCESS_KEY",
        "_ACCESS_KEY_ID",
        "_KEY_ID",
        "_PRIVATE_KEY",
        "_APIKEY",
        "_ACCESSKEY",
        "_SECRETKEY",
        "_APITOKEN",
        "_AUTH_TOKEN",
        "_TOKEN",
        "_PAT",
        "_PWD",
        "_PASS",
        "_PASSPHRASE",
        "_CREDS",
    ];
    const SENSITIVE_EXACT: &[&str] = &[
        "APIKEY",
        "API_KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "CREDENTIALS",
    ];
    const SENSITIVE_CONTAINS: &[&str] = &[
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "PRIVATE_KEY",
        "BEARER_TOKEN",
    ];

    SENSITIVE_EXACT.contains(&upper.as_str())
        || SENSITIVE_SUFFIXES
            .iter()
            .any(|suffix| upper.ends_with(suffix))
        || SENSITIVE_CONTAINS
            .iter()
            .any(|needle| upper.contains(needle))
}

fn strip_sensitive_keys(map: &mut Map<String, Value>) -> bool {
    let sensitive_keys: Vec<String> = map
        .keys()
        .filter(|key| is_sensitive_config_key(key))
        .cloned()
        .collect();
    let changed = !sensitive_keys.is_empty();
    for key in sensitive_keys {
        map.remove(&key);
    }
    changed
}

/// Claude model routes, context limits, endpoints, and credentials belong to
/// the provider. Match upstream by applying this filter when extracting a
/// common-config snippet; explicitly authored snippets keep their normal merge
/// semantics.
pub(super) fn strip_claude_provider_owned_common_fields(config: &mut Value) -> bool {
    let mut changed = false;
    let mut remove_empty_env = false;
    if let Some(env) = config.get_mut("env").and_then(Value::as_object_mut) {
        for key in claude_provider_owned_env_keys() {
            changed |= env.remove(key).is_some();
        }
        changed |= env.remove("ANTHROPIC_BASE_URL").is_some();
        remove_empty_env = env.is_empty();
    }

    if let Some(root) = config.as_object_mut() {
        if remove_empty_env {
            changed |= root.remove("env").is_some();
        }
        for key in CLAUDE_PROVIDER_OWNED_TOP_LEVEL_KEYS {
            changed |= root.remove(*key).is_some();
        }
    }
    changed
}

pub(super) fn sanitize_extracted_claude_common_config(config: &mut Value) -> bool {
    let mut changed = strip_claude_provider_owned_common_fields(config);
    let mut remove_empty_env = false;
    if let Some(env) = config.get_mut("env").and_then(Value::as_object_mut) {
        changed |= strip_sensitive_keys(env);
        remove_empty_env = env.is_empty();
    }
    if let Some(root) = config.as_object_mut() {
        if remove_empty_env {
            changed |= root.remove("env").is_some();
        }
        changed |= strip_sensitive_keys(root);
    }
    changed
}

fn provider_env_targets_gpt56(provider_env: Option<&Map<String, Value>>) -> bool {
    let Some(env) = provider_env else {
        return false;
    };

    let mut saw_model = false;
    for key in CLAUDE_ROUTED_MODEL_ENV_KEYS {
        let Some(value) = env.get(key) else {
            continue;
        };
        let Some(model) = value.as_str() else {
            return false;
        };
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        saw_model = true;
        if !model.to_ascii_lowercase().starts_with("gpt-5.6") {
            return false;
        }
    }
    saw_model
}

/// Codex OAuth exposes GPT model ids through Claude Code. Keep the catalog
/// window provider-owned and enrich only the effective live snapshot so older
/// saved providers receive the current upstream defaults without persisting
/// them as user overrides.
fn apply_codex_oauth_claude_context_defaults(settings: &mut Value, provider: &Provider) {
    if !provider.is_codex_oauth() {
        return;
    }

    let provider_env = provider
        .settings_config
        .get("env")
        .and_then(Value::as_object);
    let Some(root) = settings.as_object_mut() else {
        return;
    };
    let env = root.entry("env".to_string()).or_insert_with(|| json!({}));
    let Some(env) = env.as_object_mut() else {
        log::warn!(
            "Cannot apply Codex OAuth Claude context defaults for '{}': env is not an object",
            provider.id
        );
        return;
    };

    let inject_defaults = provider_env_targets_gpt56(provider_env);
    for key in CLAUDE_CONTEXT_WINDOW_ENV_KEYS {
        match provider_env.and_then(|provider_env| provider_env.get(key)) {
            Some(value) => {
                env.insert(key.to_string(), value.clone());
            }
            None if inject_defaults => {
                env.insert(
                    key.to_string(),
                    Value::String(CODEX_OAUTH_CONTEXT_TOKENS.to_string()),
                );
            }
            None => {
                env.remove(key);
            }
        }
    }
}

/// Remove only defaults that were injected into live settings. Explicit stored
/// values and live values changed by the user remain provider-owned.
fn strip_injected_codex_oauth_context_defaults(settings: &mut Value, provider: &Provider) {
    if !provider.is_codex_oauth() {
        return;
    }

    let provider_env = provider
        .settings_config
        .get("env")
        .and_then(Value::as_object);
    if !provider_env_targets_gpt56(provider_env) {
        return;
    }
    let Some(env) = settings.get_mut("env").and_then(Value::as_object_mut) else {
        return;
    };

    for key in CLAUDE_CONTEXT_WINDOW_ENV_KEYS {
        if provider_env.is_some_and(|provider_env| provider_env.contains_key(key)) {
            continue;
        }
        if env.get(key).and_then(Value::as_str) == Some(CODEX_OAUTH_CONTEXT_TOKENS) {
            env.remove(key);
        }
    }
}

fn restore_live_settings_for_provider_backfill(
    app_type: &AppType,
    provider: &Provider,
    mut settings: Value,
) -> Value {
    if matches!(app_type, AppType::Claude) {
        strip_injected_codex_oauth_context_defaults(&mut settings, provider);
    }
    settings
}

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

fn parse_json_object_snippet(app_type: &AppType, snippet: &str) -> Result<Value, AppError> {
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
        AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => AppError::localized(
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
            AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => AppError::localized(
                "common_config.opencode.not_object",
                "OpenCode 通用配置片段必须是 JSON 对象",
                "OpenCode common config snippet must be a JSON object",
            ),
            AppType::Codex => AppError::Config("Unexpected JSON common config type".into()),
        });
    }

    Ok(value)
}

fn parse_codex_snippet(snippet: &str) -> Result<DocumentMut, AppError> {
    snippet
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Config(format!("Common config TOML parse error: {e}")))
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
        AppType::Claude
        | AppType::Gemini
        | AppType::OpenCode
        | AppType::Hermes
        | AppType::OpenClaw => {
            parse_json_object_snippet(app_type, snippet)?;
        }
        AppType::Codex => {
            parse_codex_snippet(snippet)?;
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
        AppType::Claude => match parse_json_object_snippet(app_type, trimmed) {
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
            let source_doc = match trimmed.parse::<DocumentMut>() {
                Ok(doc) => doc,
                Err(_) => return false,
            };

            toml_item_is_subset(target_doc.as_item(), source_doc.as_item())
        }
        AppType::Gemini => match parse_json_object_snippet(app_type, trimmed) {
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
        AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => false,
    }
}

pub(super) fn provider_uses_common_config(
    app_type: &AppType,
    provider: &Provider,
    snippet: Option<&str>,
) -> bool {
    let _ = app_type;
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.apply_common_config)
        == Some(true)
        && snippet.is_some_and(|value| !value.trim().is_empty())
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
            let source = parse_json_object_snippet(app_type, trimmed)?;
            let mut result = settings.clone();
            json_deep_merge(&mut result, &source);
            Ok(result)
        }
        AppType::Codex => {
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
            let source_doc = parse_codex_snippet(trimmed)?;

            merge_toml_table_like(target_doc.as_table_mut(), source_doc.as_table());
            if let Some(obj) = result.as_object_mut() {
                obj.insert("config".to_string(), Value::String(target_doc.to_string()));
            }
            Ok(result)
        }
        AppType::Gemini => {
            let source = parse_json_object_snippet(app_type, trimmed)?;
            let mut result = settings.clone();
            if let Some(env) = result.get_mut("env") {
                json_deep_merge(env, &source);
            } else if let Some(obj) = result.as_object_mut() {
                obj.insert("env".to_string(), source);
            }
            Ok(result)
        }
        AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => Ok(settings.clone()),
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
            let source = parse_json_object_snippet(app_type, trimmed)?;
            let mut result = settings.clone();
            json_deep_remove(&mut result, &source);
            Ok(result)
        }
        AppType::Codex => {
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
            let source_doc = parse_codex_snippet(trimmed)?;

            remove_toml_table_like(target_doc.as_table_mut(), source_doc.as_table());
            if let Some(obj) = result.as_object_mut() {
                obj.insert("config".to_string(), Value::String(target_doc.to_string()));
            }
            Ok(result)
        }
        AppType::Gemini => {
            let source = parse_json_object_snippet(app_type, trimmed)?;
            let mut result = settings.clone();
            if let Some(env) = result.get_mut("env") {
                json_deep_remove(env, &source);
            }
            Ok(result)
        }
        AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => Ok(settings.clone()),
    }
}

pub(super) fn build_effective_settings_with_common_config(
    app_type: &AppType,
    provider: &Provider,
    snippet: Option<&str>,
    requested_apply_common_config: bool,
) -> Result<Value, AppError> {
    let mut effective_settings = provider.settings_config.clone();

    if requested_apply_common_config {
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

    if matches!(app_type, AppType::Claude) {
        apply_codex_oauth_claude_context_defaults(&mut effective_settings, provider);
    }

    Ok(effective_settings)
}

pub(super) fn strip_common_config_from_live_settings(
    app_type: &AppType,
    provider: &Provider,
    live_settings: Value,
    snippet: Option<&str>,
) -> Value {
    let settings = strip_common_config_from_live_settings_with_fallback(
        app_type,
        provider,
        live_settings,
        snippet,
        None,
    );
    restore_live_settings_for_provider_backfill(app_type, provider, settings)
}

#[expect(
    dead_code,
    reason = "kept for live-setting fallback stripping when provider snapshots are needed"
)]
pub(super) fn strip_common_config_from_live_settings_or_provider_snapshot(
    app_type: &AppType,
    provider: &Provider,
    live_settings: Value,
    snippet: Option<&str>,
) -> Value {
    let settings = strip_common_config_from_live_settings_with_fallback(
        app_type,
        provider,
        live_settings,
        snippet,
        Some(&provider.settings_config),
    );
    restore_live_settings_for_provider_backfill(app_type, provider, settings)
}

fn strip_common_config_snippet_from_live_settings_with_fallback(
    app_type: &AppType,
    provider: &Provider,
    live_settings: Value,
    snippet: Option<&str>,
    fallback_settings: Option<&Value>,
) -> Value {
    let Some(snippet_text) = snippet else {
        return live_settings;
    };

    if snippet_text.trim().is_empty() {
        return live_settings;
    }

    match remove_common_config_from_settings(app_type, &live_settings, snippet_text) {
        Ok(settings) => settings,
        Err(err) => {
            log::warn!(
                "Failed to strip common config for {} provider '{}': {err}",
                app_type.as_str(),
                provider.id
            );
            fallback_settings.cloned().unwrap_or(live_settings)
        }
    }
}

fn strip_common_config_from_live_settings_with_fallback(
    app_type: &AppType,
    provider: &Provider,
    live_settings: Value,
    snippet: Option<&str>,
    fallback_settings: Option<&Value>,
) -> Value {
    if !provider_uses_common_config(app_type, provider, snippet) {
        return live_settings;
    }

    strip_common_config_snippet_from_live_settings_with_fallback(
        app_type,
        provider,
        live_settings,
        snippet,
        fallback_settings,
    )
}

pub(super) fn normalize_provider_common_config_for_storage(
    app_type: &AppType,
    provider: &mut Provider,
    snippet: Option<&str>,
) -> Result<(), AppError> {
    if provider
        .meta
        .as_ref()
        .and_then(|meta| meta.apply_common_config)
        != Some(true)
    {
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

fn scrub_leaked_gemini_common_config_if_needed(
    db: &Database,
    config: &mut MultiAppConfig,
) -> Result<(), AppError> {
    let app_type = AppType::Gemini;
    if db.get_bool_flag(GEMINI_CREDENTIAL_SCRUB_MARKER)? {
        return Ok(());
    }

    let Some(snippet_text) = db.get_config_snippet(app_type.as_str())? else {
        db.set_setting(GEMINI_CREDENTIAL_SCRUB_MARKER, "true")?;
        return Ok(());
    };

    let Ok(Value::Object(entries)) = serde_json::from_str::<Value>(&snippet_text) else {
        db.set_setting(GEMINI_CREDENTIAL_SCRUB_MARKER, "true")?;
        return Ok(());
    };

    let mut poison = Map::new();
    let mut clean = Map::new();
    for (key, value) in entries {
        if is_sensitive_config_key(&key) {
            poison.insert(key, value);
        } else {
            clean.insert(key, value);
        }
    }

    if poison.is_empty() {
        db.set_setting(GEMINI_CREDENTIAL_SCRUB_MARKER, "true")?;
        return Ok(());
    }

    let poison_keys: Vec<String> = poison.keys().cloned().collect();
    let poison_value = Value::Object(poison);
    let poison_settings = json!({ "env": poison_value.clone() });

    // Compute every provider cleanup from the raw leaked values. The regular
    // snippet parser intentionally strips credentials and cannot be used here.
    let providers = db.get_all_providers(app_type.as_str())?;
    let mut pending = Vec::new();
    for (id, provider) in providers {
        let mut cleaned = provider.settings_config.clone();
        json_deep_remove(&mut cleaned, &poison_settings);
        if cleaned != provider.settings_config {
            pending.push((id, provider, cleaned));
        }
    }

    let removed_env_keys = |before: &Value, after: &Value| -> Vec<String> {
        let before_env = before.get("env").and_then(Value::as_object);
        let after_env = after.get("env").and_then(Value::as_object);
        match (before_env, after_env) {
            (Some(before_env), Some(after_env)) => before_env
                .keys()
                .filter(|key| !after_env.contains_key(*key))
                .cloned()
                .collect(),
            (Some(before_env), None) => before_env.keys().cloned().collect(),
            _ => Vec::new(),
        }
    };
    let audit = json!({
        "removedFromSnippet": poison_keys,
        "providers": pending
            .iter()
            .map(|(id, provider, cleaned)| json!({
                "id": id,
                "removedKeys": removed_env_keys(&provider.settings_config, cleaned),
            }))
            .collect::<Vec<_>>(),
    });
    if db.get_setting(GEMINI_CREDENTIAL_SCRUB_AUDIT_KEY)?.is_none() {
        let audit_text = serde_json::to_string(&audit)
            .map_err(|error| AppError::Message(format!("Serialization failed: {error}")))?;
        db.set_setting(GEMINI_CREDENTIAL_SCRUB_AUDIT_KEY, &audit_text)?;
    }

    for (id, mut provider, cleaned) in pending {
        provider.settings_config = cleaned;
        db.save_provider(app_type.as_str(), &provider)?;
        config.ensure_app(&app_type);
        if let Some(manager) = config.get_manager_mut(&app_type) {
            manager.providers.insert(id, provider);
        }
    }

    // A takeover backup can restore the leaked value later, so clean it before
    // the snippet that identifies the poisoned key/value pair is replaced.
    if let Some(backup) = futures::executor::block_on(db.get_live_backup(app_type.as_str()))? {
        let mut original: Value = serde_json::from_str(&backup.original_config)
            .map_err(|error| AppError::Message(format!("Invalid Gemini live backup: {error}")))?;
        let before = original.clone();
        json_deep_remove(&mut original, &poison_settings);
        if original != before {
            let text = serde_json::to_string(&original)
                .map_err(|error| AppError::Message(format!("Serialization failed: {error}")))?;
            futures::executor::block_on(db.save_live_backup(app_type.as_str(), &text))?;
        }
    }

    let poison_env: HashMap<String, String> = poison_value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    crate::gemini_config::remove_gemini_env_entries(&poison_env)?;

    // The snippet is the only source that identifies injected values, so it is
    // deliberately updated after providers, backups, and the live env.
    let cleaned_snippet = if clean.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string_pretty(&Value::Object(clean))
                .map_err(|error| AppError::Message(format!("Serialization failed: {error}")))?,
        )
    };
    db.set_config_snippet(app_type.as_str(), cleaned_snippet.clone())?;
    config
        .common_config_snippets
        .set(&app_type, cleaned_snippet);
    db.set_setting(GEMINI_CREDENTIAL_SCRUB_MARKER, "true")?;
    Ok(())
}

pub(crate) fn migrate_common_config_upstream_semantics_if_needed(
    db: &Database,
    config: &mut MultiAppConfig,
) -> Result<(), AppError> {
    if let Err(error) = scrub_leaked_gemini_common_config_if_needed(db, config) {
        log::warn!(
            "skip incomplete Gemini common-config credential cleanup; it will retry: {error}"
        );
    }

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
                && settings_contain_common_config(&app_type, &provider.settings_config, &snippet)
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
