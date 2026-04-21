use super::*;

pub fn migrate_legacy_codex_config(cfg_text: &str, provider: &Provider) -> Option<String> {
    let trimmed = cfg_text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let table: toml::Table = match toml::from_str(trimmed) {
        Ok(t) => t,
        Err(_) => return None, // unparseable → leave as-is
    };

    // Already in new format
    if table.contains_key("model_provider") {
        return None;
    }

    // Detect legacy: root-level base_url or wire_api without model_provider
    let has_legacy_keys = table.contains_key("base_url") || table.contains_key("wire_api");
    if !has_legacy_keys {
        return None;
    }

    // Extract fields from legacy flat format
    let base_url = table
        .get("base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let model = table
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("gpt-5.4")
        .trim();
    let wire_api = table
        .get("wire_api")
        .and_then(|v| v.as_str())
        .unwrap_or("responses")
        .trim();
    let requires_openai_auth = table
        .get("requires_openai_auth")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let env_key = table.get("env_key").and_then(|v| v.as_str());

    // Generate provider key from provider id/name
    let raw_key = if provider.id.trim().is_empty() {
        &provider.name
    } else {
        &provider.id
    };
    let provider_key = crate::codex_config::clean_codex_provider_key(raw_key);

    // Preserve non-provider-specific root keys (model_reasoning_effort, disable_response_storage, etc.)
    let mut extra_root_lines = Vec::new();
    for (key, val) in &table {
        match key.as_str() {
            "base_url" | "model" | "wire_api" | "requires_openai_auth" | "env_key" | "name" => {
                continue
            }
            _ => {
                // Re-serialize the value as a TOML line
                if let Ok(s) = toml::to_string(&toml::Value::Table({
                    let mut t = toml::Table::new();
                    t.insert(key.clone(), val.clone());
                    t
                })) {
                    extra_root_lines.push(s.trim().to_string());
                }
            }
        }
    }

    // Build new format
    let mut lines = Vec::new();
    lines.push(format!("model_provider = \"{}\"", provider_key));
    lines.push(format!("model = \"{}\"", model));
    lines.extend(extra_root_lines);
    lines.push(String::new());
    lines.push(format!("[model_providers.{}]", provider_key));
    lines.push(format!("name = \"{}\"", provider_key));
    if !base_url.is_empty() {
        lines.push(format!("base_url = \"{}\"", base_url));
    }
    lines.push(format!("wire_api = \"{}\"", wire_api));
    if requires_openai_auth {
        lines.push("requires_openai_auth = true".to_string());
    } else {
        lines.push("requires_openai_auth = false".to_string());
        if let Some(ek) = env_key {
            let ek = ek.trim();
            if !ek.is_empty() {
                lines.push(format!("env_key = \"{}\"", ek));
            }
        }
    }
    lines.push(String::new());

    log::info!(
        "Migrated legacy Codex config for provider '{}' to model_provider format",
        provider.id
    );
    Some(lines.join("\n"))
}

/// Strip common config snippet keys from a full Codex config.toml text.
///
/// When storing a provider snapshot, we remove keys that belong to the common
/// config snippet so they don't get duplicated when the common snippet is
/// merged back in during `write_codex_live`.
pub(super) fn strip_codex_common_config_from_full_text(
    config_text: &str,
    common_snippet: &str,
) -> Result<String, AppError> {
    if common_snippet.trim().is_empty() || config_text.trim().is_empty() {
        return Ok(config_text.to_string());
    }

    let common_doc = common_snippet
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| AppError::Config(format!("Common config TOML parse error: {e}")))?;
    if common_doc.as_table().is_empty() {
        return Ok(config_text.to_string());
    }

    let mut doc = config_text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| AppError::Config(format!("TOML parse error: {e}")))?;

    for (key, common_item) in common_doc.as_table().iter() {
        // Strip all common keys EXCEPT provider-identity keys.
        match key {
            "model" | "model_provider" | "model_providers" => continue,
            _ => {
                let mut single_key_table = toml_edit::Table::new();
                single_key_table.insert(key, common_item.clone());
                ProviderService::strip_toml_tables(doc.as_table_mut(), &single_key_table);
            }
        }
    }

    Ok(doc.to_string())
}

pub(super) fn merge_json_values(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                match base_map.get_mut(key) {
                    Some(base_value) => merge_json_values(base_value, overlay_value),
                    None => {
                        base_map.insert(key.clone(), overlay_value.clone());
                    }
                }
            }
        }
        (base_value, overlay_value) => {
            *base_value = overlay_value.clone();
        }
    }
}

pub(super) fn strip_common_values(target: &mut Value, common: &Value) {
    match (target, common) {
        (Value::Object(target_map), Value::Object(common_map)) => {
            for (key, common_value) in common_map {
                let should_remove = match target_map.get_mut(key) {
                    Some(target_value) => match target_value {
                        Value::Object(_) if matches!(common_value, Value::Object(_)) => {
                            strip_common_values(target_value, common_value);
                            target_value.as_object().is_some_and(|m| m.is_empty())
                        }
                        _ => target_value == common_value,
                    },
                    None => false,
                };

                if should_remove {
                    target_map.remove(key);
                }
            }
        }
        (target_value, common_value) => {
            if target_value == common_value {
                *target_value = Value::Null;
            }
        }
    }
}
