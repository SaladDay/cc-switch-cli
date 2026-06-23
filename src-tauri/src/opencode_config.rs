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

#[allow(dead_code)]
pub fn set_provider(id: &str, provider: Value) -> Result<(), AppError> {
    set_provider_value(id, provider, true)
}

fn set_provider_value(
    id: &str,
    mut provider: Value,
    preserve_modalities: bool,
) -> Result<(), AppError> {
    let mut full_config = read_opencode_config()?;

    if full_config.get("provider").is_none() {
        full_config["provider"] = json!({});
    }

    if let Some(providers) = full_config
        .get_mut("provider")
        .and_then(Value::as_object_mut)
    {
        if preserve_modalities {
            if let Some(incoming) = provider.as_object_mut() {
                if !incoming.contains_key("modalities") {
                    if let Some(modalities) = providers
                        .get(id)
                        .and_then(Value::as_object)
                        .and_then(|existing| existing.get("modalities"))
                        .cloned()
                    {
                        incoming.insert("modalities".to_string(), modalities);
                    }
                }
            }
        }

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
    prepare_provider_with_base_and_resolution(id, None, provider, resolution)
}

pub fn prepare_provider_with_base_and_resolution(
    id: &str,
    base_provider: Option<Value>,
    provider: Value,
    resolution: live_merge::ConflictResolution<'_>,
) -> Result<Value, AppError> {
    prepare_provider_with_base_deleted_keys_and_resolution(
        id,
        base_provider,
        provider,
        &[],
        resolution,
    )
}

fn prepare_provider_with_base_deleted_keys_and_resolution(
    id: &str,
    base_provider: Option<Value>,
    provider: Value,
    deleted_keys: &[&str],
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
            Some(existing) => match prepare_base_for_deleted_keys(
                base_provider,
                existing,
                &provider,
                deleted_keys,
            ) {
                Some(base_provider) => live_merge::merge_json_with_base_live(
                    &crate::app_config::AppType::OpenCode,
                    format!("opencode.json provider.{id}"),
                    existing.clone(),
                    &base_provider,
                    &provider,
                    resolution,
                )?,
                None => live_merge::merge_json_live(
                    &crate::app_config::AppType::OpenCode,
                    format!("opencode.json provider.{id}"),
                    existing.clone(),
                    &provider,
                    resolution,
                )?,
            },
            None => provider,
        };
        providers.insert(id.to_string(), merged);
    }

    Ok(full_config)
}

fn prepare_base_for_deleted_keys(
    base_provider: Option<Value>,
    existing: &Value,
    provider: &Value,
    deleted_keys: &[&str],
) -> Option<Value> {
    if deleted_keys.is_empty() {
        return base_provider;
    }

    let mut base_provider = base_provider.unwrap_or_else(|| json!({}));
    let Some(base_object) = base_provider.as_object_mut() else {
        return Some(base_provider);
    };
    let existing_object = existing.as_object();
    let provider_object = provider.as_object();
    for key in deleted_keys {
        if provider_object.is_some_and(|object| object.contains_key(*key))
            || base_object.contains_key(*key)
        {
            continue;
        }
        if let Some(existing_value) = existing_object.and_then(|object| object.get(*key)) {
            base_object.insert((*key).to_string(), existing_value.clone());
        }
    }

    Some(base_provider)
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

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "kept for direct typed OpenCode provider writes")
)]
pub fn set_typed_provider(id: &str, config: &OpenCodeProviderConfig) -> Result<(), AppError> {
    let value =
        serde_json::to_value(config).map_err(|source| AppError::JsonSerialize { source })?;
    set_provider_value(id, value, false)
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

#[expect(
    dead_code,
    reason = "kept for direct typed OpenCode provider writes with conflict resolution"
)]
pub fn prepare_typed_provider_with_resolution(
    id: &str,
    config: &OpenCodeProviderConfig,
    resolution: live_merge::ConflictResolution<'_>,
) -> Result<Value, AppError> {
    prepare_typed_provider_with_base_and_resolution(id, None, config, resolution)
}

pub fn prepare_typed_provider_with_base_and_resolution(
    id: &str,
    base_config: Option<&OpenCodeProviderConfig>,
    config: &OpenCodeProviderConfig,
    resolution: live_merge::ConflictResolution<'_>,
) -> Result<Value, AppError> {
    let base_value = base_config
        .map(serde_json::to_value)
        .transpose()
        .map_err(|source| AppError::JsonSerialize { source })?;
    let value =
        serde_json::to_value(config).map_err(|source| AppError::JsonSerialize { source })?;
    let mut deleted_keys = Vec::new();
    if config.name.is_none() {
        deleted_keys.push("name");
    }
    if config.modalities.is_none() {
        deleted_keys.push("modalities");
    }
    prepare_provider_with_base_deleted_keys_and_resolution(
        id,
        base_value,
        value,
        &deleted_keys,
        resolution,
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use serde_json::json;

    fn provider_without_modalities(base_url: &str) -> Value {
        json!({
            "npm": OPENCODE_DEFAULT_NPM,
            "options": {
                "baseURL": base_url
            }
        })
    }

    fn seed_provider_with_modalities(modalities: &Value) {
        write_opencode_config(&json!({
            "$schema": "https://opencode.ai/config.json",
            "provider": {
                "vision": {
                    "npm": OPENCODE_DEFAULT_NPM,
                    "options": {
                        "baseURL": "https://old.example.com/v1"
                    },
                    "modalities": modalities
                }
            }
        }))
        .expect("seed opencode config");
    }

    #[test]
    fn opencode_provider_config_raw_set_provider_preserves_modalities_on_omit() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let _env = TestEnvGuard::isolated(temp.path());
        let modalities = json!({ "input": ["text", "image"] });
        seed_provider_with_modalities(&modalities);

        set_provider(
            "vision",
            provider_without_modalities("https://new.example.com/v1"),
        )
        .expect("set raw provider");

        let live = read_opencode_config().expect("read opencode config");
        assert_eq!(
            live["provider"]["vision"]["options"]["baseURL"],
            json!("https://new.example.com/v1")
        );
        assert_eq!(live["provider"]["vision"]["modalities"], modalities);
    }

    #[test]
    fn opencode_provider_config_typed_set_provider_can_clear_modalities() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let _env = TestEnvGuard::isolated(temp.path());
        let modalities = json!({ "input": ["text", "image"] });
        seed_provider_with_modalities(&modalities);
        let config: OpenCodeProviderConfig =
            serde_json::from_value(provider_without_modalities("https://new.example.com/v1"))
                .expect("deserialize typed provider");

        set_typed_provider("vision", &config).expect("set typed provider");

        let live = read_opencode_config().expect("read opencode config");
        let provider = live["provider"]["vision"]
            .as_object()
            .expect("serialized provider object");
        assert_eq!(
            provider["options"]["baseURL"],
            json!("https://new.example.com/v1")
        );
        assert!(!provider.contains_key("modalities"));
    }

    #[test]
    fn opencode_provider_config_typed_prepare_can_clear_modalities_from_live_only_base() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let _env = TestEnvGuard::isolated(temp.path());
        let modalities = json!({ "input": ["text", "image"] });
        seed_provider_with_modalities(&modalities);
        let config: OpenCodeProviderConfig =
            serde_json::from_value(provider_without_modalities("https://new.example.com/v1"))
                .expect("deserialize typed provider");
        let base_config: OpenCodeProviderConfig =
            serde_json::from_value(provider_without_modalities("https://old.example.com/v1"))
                .expect("deserialize typed provider");

        let prepared = prepare_typed_provider_with_base_and_resolution(
            "vision",
            Some(&base_config),
            &config,
            live_merge::ConflictPolicy::Fail.into(),
        )
        .expect("prepare typed provider");
        let provider = prepared["provider"]["vision"]
            .as_object()
            .expect("serialized provider object");

        assert_eq!(
            provider["options"]["baseURL"],
            json!("https://new.example.com/v1")
        );
        assert!(!provider.contains_key("modalities"));
    }
}
