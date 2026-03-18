use crate::config::write_json_file;
use crate::error::AppError;
use crate::settings::get_openclaw_override_dir;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

pub fn get_openclaw_dir() -> PathBuf {
    if let Some(override_dir) = get_openclaw_override_dir() {
        return override_dir;
    }

    dirs::home_dir()
        .map(|home| home.join(".openclaw"))
        .unwrap_or_else(|| PathBuf::from(".openclaw"))
}

pub fn get_openclaw_workspace_dir() -> PathBuf {
    get_openclaw_dir().join("workspace")
}

pub fn get_openclaw_config_path() -> PathBuf {
    get_openclaw_dir().join("openclaw.json")
}

pub fn read_openclaw_config() -> Result<Value, AppError> {
    let path = get_openclaw_config_path();
    if !path.exists() {
        return Ok(json!({}));
    }

    let content = std::fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
    json5::from_str(&content).map_err(|e| {
        AppError::Config(format!(
            "OpenClaw config parse error: {}: {e}",
            path.display()
        ))
    })
}

pub fn write_openclaw_config(config: &Value) -> Result<(), AppError> {
    let path = get_openclaw_config_path();
    write_json_file(&path, config)
}

pub fn get_providers() -> Result<Map<String, Value>, AppError> {
    let config = read_openclaw_config()?;
    Ok(config
        .get("models")
        .and_then(|value| value.get("providers"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default())
}

pub fn set_provider(id: &str, provider: Value) -> Result<(), AppError> {
    let mut config = read_openclaw_config()?;
    let providers = ensure_object_path(&mut config, &["models", "providers"])?;
    providers.insert(id.to_string(), provider);
    write_openclaw_config(&config)
}

pub fn remove_provider(id: &str) -> Result<(), AppError> {
    let mut config = read_openclaw_config()?;
    if let Some(providers) = config
        .get_mut("models")
        .and_then(|value| value.get_mut("providers"))
        .and_then(Value::as_object_mut)
    {
        providers.remove(id);
    }
    write_openclaw_config(&config)
}

pub fn get_primary_model() -> Result<Option<String>, AppError> {
    let config = read_openclaw_config()?;
    Ok(primary_model_from_config(&config))
}

pub fn set_primary_model(model: &str) -> Result<(), AppError> {
    let mut config = read_openclaw_config()?;
    apply_primary_model(&mut config, model);
    write_openclaw_config(&config)
}

pub fn ensure_model_allowlist_entry(model: &str) -> Result<(), AppError> {
    let mut config = read_openclaw_config()?;
    ensure_model_entry_in_config(&mut config, model);
    write_openclaw_config(&config)
}

pub fn provider_config_from_settings(settings_config: &Value) -> Value {
    settings_config
        .get("provider")
        .cloned()
        .unwrap_or_else(|| settings_config.clone())
}

pub fn primary_model_from_settings(settings_config: &Value) -> Option<String> {
    settings_config
        .get("primaryModel")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn build_settings_config(provider_config: Value, primary_model: Option<String>) -> Value {
    let mut root = Map::new();
    root.insert("provider".to_string(), provider_config);
    if let Some(primary_model) = primary_model.filter(|value| !value.trim().is_empty()) {
        root.insert("primaryModel".to_string(), Value::String(primary_model));
    }
    Value::Object(root)
}

pub fn infer_primary_model(provider_id: &str, provider_config: &Value) -> Option<String> {
    let model_id = provider_config
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| models.first())
        .and_then(|model| model.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    Some(format!("{provider_id}/{model_id}"))
}

pub fn primary_model_from_config(config: &Value) -> Option<String> {
    let model = config
        .get("agents")
        .and_then(|value| value.get("defaults"))
        .and_then(|value| value.get("model"))?;

    if let Some(value) = model.as_str() {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    model
        .get("primary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn apply_provider_snapshot(provider_id: &str, settings_config: &Value) -> Result<(), AppError> {
    let mut config = read_openclaw_config()?;
    let provider = provider_config_from_settings(settings_config);
    let providers = ensure_object_path(&mut config, &["models", "providers"])?;
    providers.insert(provider_id.to_string(), provider);

    if let Some(primary_model) = primary_model_from_settings(settings_config) {
        apply_primary_model(&mut config, &primary_model);
        ensure_model_entry_in_config(&mut config, &primary_model);
    }

    write_openclaw_config(&config)
}

fn apply_primary_model(config: &mut Value, model: &str) {
    let model_root = ensure_object_path_mut(config, &["agents", "defaults"]);
    if let Some(existing) = model_root.get_mut("model") {
        if let Some(existing_obj) = existing.as_object_mut() {
            existing_obj.insert("primary".to_string(), Value::String(model.to_string()));
            return;
        }
    }
    model_root.insert("model".to_string(), json!({ "primary": model }));
}

fn ensure_model_entry_in_config(config: &mut Value, model: &str) {
    let models = ensure_object_path_mut(config, &["agents", "defaults", "models"]);
    models
        .entry(model.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
}

fn ensure_object_path<'a>(
    root: &'a mut Value,
    path: &[&str],
) -> Result<&'a mut Map<String, Value>, AppError> {
    if !root.is_object() {
        *root = Value::Object(Map::new());
    }
    Ok(ensure_object_path_mut(root, path))
}

fn ensure_object_path_mut<'a>(root: &'a mut Value, path: &[&str]) -> &'a mut Map<String, Value> {
    let mut current = root;
    for segment in path {
        if !current.is_object() {
            *current = Value::Object(Map::new());
        }
        let object = current
            .as_object_mut()
            .expect("object ensured before descending");
        current = object
            .entry((*segment).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if !current.is_object() {
        *current = Value::Object(Map::new());
    }
    current
        .as_object_mut()
        .expect("terminal object ensured before returning")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_settings_config_wraps_provider_and_primary_model() {
        let settings = build_settings_config(
            json!({ "baseUrl": "https://api.example.com/v1" }),
            Some("demo/model".to_string()),
        );

        assert_eq!(
            settings
                .get("provider")
                .and_then(|value| value.get("baseUrl")),
            Some(&Value::String("https://api.example.com/v1".to_string()))
        );
        assert_eq!(
            primary_model_from_settings(&settings).as_deref(),
            Some("demo/model")
        );
    }

    #[test]
    fn infer_primary_model_uses_first_provider_model() {
        let provider = json!({
            "models": [
                { "id": "gpt-5.4", "name": "GPT-5.4" },
                { "id": "gpt-5.3", "name": "GPT-5.3" }
            ]
        });

        assert_eq!(
            infer_primary_model("demo", &provider).as_deref(),
            Some("demo/gpt-5.4")
        );
    }

    #[test]
    fn primary_model_from_config_supports_object_form() {
        let config = json!({
            "agents": {
                "defaults": {
                    "model": {
                        "primary": "demo/model"
                    }
                }
            }
        });

        assert_eq!(
            primary_model_from_config(&config).as_deref(),
            Some("demo/model")
        );
    }
}
