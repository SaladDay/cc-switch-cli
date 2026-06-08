use crate::config::write_json_file;
use crate::error::AppError;
use crate::provider::OpenCodeProviderConfig;
use crate::services::provider::live_merge;
use crate::settings::get_opencode_override_dir;
use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

pub const OPENCODE_DEFAULT_NPM: &str = "@ai-sdk/openai-compatible";

pub fn get_opencode_dir() -> PathBuf {
    if let Some(override_dir) = get_opencode_override_dir() {
        return override_dir;
    }

    dirs::home_dir()
        .map(|home| home.join(".config").join("opencode"))
        .unwrap_or_else(|| PathBuf::from(".config").join("opencode"))
}

pub fn get_opencode_config_path() -> PathBuf {
    get_opencode_dir().join("opencode.json")
}

pub fn get_opencode_base_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("opencode");
        }
    }

    dirs::home_dir()
        .map(|home| home.join(".local").join("share").join("opencode"))
        .unwrap_or_else(|| PathBuf::from(".local").join("share").join("opencode"))
}

pub fn get_opencode_db_path() -> PathBuf {
    get_opencode_base_dir().join("opencode.db")
}

pub fn read_opencode_config() -> Result<Value, AppError> {
    let path = get_opencode_config_path();
    if !path.exists() {
        return Ok(json!({ "$schema": "https://opencode.ai/config.json" }));
    }

    let content = std::fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
    serde_json::from_str(&content).map_err(|e| AppError::json(&path, e))
}

pub fn write_opencode_config(config: &Value) -> Result<(), AppError> {
    let path = get_opencode_config_path();
    write_json_file(&path, config)
}

pub fn get_providers() -> Result<Map<String, Value>, AppError> {
    let config = read_opencode_config()?;
    Ok(config
        .get("provider")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default())
}

pub fn set_provider(id: &str, provider: Value) -> Result<(), AppError> {
    let mut full_config = read_opencode_config()?;

    if full_config.get("provider").is_none() {
        full_config["provider"] = json!({});
    }

    if let Some(providers) = full_config
        .get_mut("provider")
        .and_then(Value::as_object_mut)
    {
        providers.insert(id.to_string(), provider);
    }

    write_opencode_config(&full_config)
}

pub fn set_provider_with_resolution(
    id: &str,
    provider: Value,
    resolution: live_merge::ConflictResolution<'_>,
) -> Result<(), AppError> {
    let full_config = prepare_provider_with_resolution(id, provider, resolution)?;
    write_opencode_config(&full_config)
}

pub fn prepare_provider_with_resolution(
    id: &str,
    provider: Value,
    resolution: live_merge::ConflictResolution<'_>,
) -> Result<Value, AppError> {
    let mut full_config = read_opencode_config()?;

    if full_config.get("provider").is_none() {
        full_config["provider"] = json!({});
    }

    if let Some(providers) = full_config
        .get_mut("provider")
        .and_then(Value::as_object_mut)
    {
        let merged = match providers.get(id) {
            Some(existing) => live_merge::merge_json_live(
                &crate::app_config::AppType::OpenCode,
                format!("opencode.json provider.{id}"),
                existing.clone(),
                &provider,
                resolution,
            )?,
            None => provider,
        };
        providers.insert(id.to_string(), merged);
    }

    Ok(full_config)
}

pub fn write_prepared_config(config: &Value) -> Result<(), AppError> {
    write_opencode_config(config)
}

pub fn remove_provider(id: &str) -> Result<(), AppError> {
    let mut full_config = read_opencode_config()?;
    if let Some(providers) = full_config
        .get_mut("provider")
        .and_then(Value::as_object_mut)
    {
        providers.remove(id);
    }
    write_opencode_config(&full_config)
}

pub fn get_typed_providers() -> Result<IndexMap<String, OpenCodeProviderConfig>, AppError> {
    let mut result = IndexMap::new();
    for (id, value) in get_providers()? {
        match serde_json::from_value::<OpenCodeProviderConfig>(value) {
            Ok(config) => {
                result.insert(id, config);
            }
            Err(err) => {
                log::warn!("Failed to parse OpenCode provider '{id}': {err}");
            }
        }
    }
    Ok(result)
}

#[expect(dead_code, reason = "kept for direct typed OpenCode provider writes")]
pub fn set_typed_provider(id: &str, config: &OpenCodeProviderConfig) -> Result<(), AppError> {
    let value =
        serde_json::to_value(config).map_err(|source| AppError::JsonSerialize { source })?;
    set_provider(id, value)
}

#[expect(
    dead_code,
    reason = "kept for direct typed OpenCode provider writes with conflict resolution"
)]
pub fn set_typed_provider_with_resolution(
    id: &str,
    config: &OpenCodeProviderConfig,
    resolution: live_merge::ConflictResolution<'_>,
) -> Result<(), AppError> {
    let value =
        serde_json::to_value(config).map_err(|source| AppError::JsonSerialize { source })?;
    set_provider_with_resolution(id, value, resolution)
}

pub fn prepare_typed_provider_with_resolution(
    id: &str,
    config: &OpenCodeProviderConfig,
    resolution: live_merge::ConflictResolution<'_>,
) -> Result<Value, AppError> {
    let value =
        serde_json::to_value(config).map_err(|source| AppError::JsonSerialize { source })?;
    prepare_provider_with_resolution(id, value, resolution)
}

pub fn get_mcp_servers() -> Result<Map<String, Value>, AppError> {
    let config = read_opencode_config()?;
    Ok(config
        .get("mcp")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default())
}

pub fn set_mcp_server(id: &str, server: Value) -> Result<(), AppError> {
    let mut full_config = read_opencode_config()?;

    if full_config.get("mcp").is_none() {
        full_config["mcp"] = json!({});
    }

    if let Some(mcp) = full_config.get_mut("mcp").and_then(Value::as_object_mut) {
        mcp.insert(id.to_string(), server);
    }

    write_opencode_config(&full_config)
}

pub fn remove_mcp_server(id: &str) -> Result<(), AppError> {
    let mut config = read_opencode_config()?;

    if let Some(mcp) = config.get_mut("mcp").and_then(Value::as_object_mut) {
        mcp.remove(id);
    }

    write_opencode_config(&config)
}
