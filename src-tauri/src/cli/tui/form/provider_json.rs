use crate::app_config::AppType;
use serde_json::{json, Value};

use super::codex_config::{
    build_codex_provider_config_toml, clean_codex_provider_key, merge_codex_common_config_snippet,
    strip_codex_common_config_snippet, update_codex_config_snippet,
};
use super::{ClaudeApiFormat, GeminiAuthType, ProviderAddFormState};

impl ProviderAddFormState {
    pub fn to_provider_json_value(&self) -> Value {
        let mut provider_obj = match self.extra.clone() {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };

        provider_obj.insert("id".to_string(), json!(self.id.value.trim()));
        provider_obj.insert("name".to_string(), json!(self.name.value.trim()));

        upsert_optional_trimmed(
            &mut provider_obj,
            "websiteUrl",
            self.website_url.value.as_str(),
        );
        upsert_optional_trimmed(&mut provider_obj, "notes", self.notes.value.as_str());

        let meta_value = provider_obj
            .entry("meta".to_string())
            .or_insert_with(|| json!({}));
        if !meta_value.is_object() {
            *meta_value = json!({});
        }
        if let Some(meta_obj) = meta_value.as_object_mut() {
            meta_obj.insert(
                "applyCommonConfig".to_string(),
                json!(self.include_common_config),
            );
            if matches!(self.app_type, AppType::Claude) {
                match self.claude_api_format {
                    _ if self.is_claude_official_provider() => {
                        meta_obj.remove("apiFormat");
                    }
                    ClaudeApiFormat::Anthropic => {
                        meta_obj.remove("apiFormat");
                    }
                    ClaudeApiFormat::OpenAiChat => {
                        meta_obj.insert("apiFormat".to_string(), json!("openai_chat"));
                    }
                    ClaudeApiFormat::OpenAiResponses => {
                        meta_obj.insert("apiFormat".to_string(), json!("openai_responses"));
                    }
                }
            }
        }

        let settings_value = provider_obj
            .entry("settingsConfig".to_string())
            .or_insert_with(|| json!({}));
        if !settings_value.is_object() {
            *settings_value = json!({});
        }
        let settings_obj = settings_value
            .as_object_mut()
            .expect("settingsConfig must be a JSON object");

        match self.app_type {
            AppType::Claude => {
                let env_value = settings_obj
                    .entry("env".to_string())
                    .or_insert_with(|| json!({}));
                if !env_value.is_object() {
                    *env_value = json!({});
                }
                let env_obj = env_value
                    .as_object_mut()
                    .expect("env must be a JSON object");
                set_or_remove_trimmed(env_obj, "ANTHROPIC_AUTH_TOKEN", &self.claude_api_key.value);
                set_or_remove_trimmed(env_obj, "ANTHROPIC_BASE_URL", &self.claude_base_url.value);
                if self.claude_model_config_touched {
                    set_or_remove_trimmed(env_obj, "ANTHROPIC_MODEL", &self.claude_model.value);
                    set_or_remove_trimmed(
                        env_obj,
                        "ANTHROPIC_REASONING_MODEL",
                        &self.claude_reasoning_model.value,
                    );
                    set_or_remove_trimmed(
                        env_obj,
                        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                        &self.claude_haiku_model.value,
                    );
                    set_or_remove_trimmed(
                        env_obj,
                        "ANTHROPIC_DEFAULT_SONNET_MODEL",
                        &self.claude_sonnet_model.value,
                    );
                    set_or_remove_trimmed(
                        env_obj,
                        "ANTHROPIC_DEFAULT_OPUS_MODEL",
                        &self.claude_opus_model.value,
                    );
                    env_obj.remove("ANTHROPIC_SMALL_FAST_MODEL");
                }
                settings_obj.remove("api_format");
                settings_obj.remove("openrouter_compat_mode");
            }
            AppType::Codex => {
                let provider_key =
                    clean_codex_provider_key(self.id.value.trim(), self.name.value.trim());
                let base_url = self.codex_base_url.value.trim().trim_end_matches('/');
                let model = if self.codex_model.is_blank() {
                    "gpt-5.2-codex"
                } else {
                    self.codex_model.value.trim()
                };

                let existing_config = settings_obj
                    .get("config")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let base_config = if existing_config.trim().is_empty() {
                    build_codex_provider_config_toml(
                        &provider_key,
                        base_url,
                        model,
                        self.codex_wire_api,
                    )
                } else {
                    existing_config.to_string()
                };
                let config_toml = update_codex_config_snippet(
                    &base_config,
                    base_url,
                    model,
                    self.codex_wire_api,
                    self.codex_requires_openai_auth,
                    self.codex_env_key.value.trim(),
                );
                settings_obj.insert("config".to_string(), Value::String(config_toml));

                if self.is_codex_official_provider() {
                    settings_obj.remove("auth");
                } else {
                    let api_key = self.codex_api_key.value.trim();
                    if api_key.is_empty() {
                        if let Some(auth_obj) = settings_obj
                            .get_mut("auth")
                            .and_then(|value| value.as_object_mut())
                        {
                            auth_obj.remove("OPENAI_API_KEY");
                            if auth_obj.is_empty() {
                                settings_obj.remove("auth");
                            }
                        } else {
                            settings_obj.remove("auth");
                        }
                    } else {
                        let auth_value = settings_obj
                            .entry("auth".to_string())
                            .or_insert_with(|| json!({}));
                        if !auth_value.is_object() {
                            *auth_value = json!({});
                        }
                        let auth_obj = auth_value
                            .as_object_mut()
                            .expect("auth must be a JSON object");
                        auth_obj.insert("OPENAI_API_KEY".to_string(), json!(api_key));
                    }
                }
            }
            AppType::Gemini => {
                let env_value = settings_obj
                    .entry("env".to_string())
                    .or_insert_with(|| json!({}));
                if !env_value.is_object() {
                    *env_value = json!({});
                }
                let env_obj = env_value
                    .as_object_mut()
                    .expect("env must be a JSON object");

                match self.gemini_auth_type {
                    GeminiAuthType::OAuth => {
                        env_obj.remove("GEMINI_API_KEY");
                        env_obj.remove("GOOGLE_GEMINI_BASE_URL");
                        env_obj.remove("GEMINI_BASE_URL");
                        env_obj.remove("GEMINI_MODEL");
                    }
                    GeminiAuthType::ApiKey => {
                        set_or_remove_trimmed(
                            env_obj,
                            "GEMINI_API_KEY",
                            &self.gemini_api_key.value,
                        );
                        set_or_remove_trimmed(
                            env_obj,
                            "GOOGLE_GEMINI_BASE_URL",
                            &self.gemini_base_url.value,
                        );
                        set_or_remove_trimmed(env_obj, "GEMINI_MODEL", &self.gemini_model.value);
                    }
                }
            }
            AppType::OpenCode => {
                let npm_package = self.opencode_npm_package.value.trim();
                settings_obj.insert(
                    "npm".to_string(),
                    json!(if npm_package.is_empty() {
                        "@ai-sdk/openai-compatible"
                    } else {
                        npm_package
                    }),
                );

                let options_value = settings_obj
                    .entry("options".to_string())
                    .or_insert_with(|| json!({}));
                if !options_value.is_object() {
                    *options_value = json!({});
                }
                let options_obj = options_value
                    .as_object_mut()
                    .expect("options must be a JSON object");
                set_or_remove_trimmed(options_obj, "apiKey", &self.opencode_api_key.value);
                set_or_remove_trimmed(options_obj, "baseURL", &self.opencode_base_url.value);
                if options_obj.is_empty() {
                    settings_obj.remove("options");
                }

                let mut models_value = settings_obj
                    .remove("models")
                    .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                if !models_value.is_object() {
                    models_value = Value::Object(serde_json::Map::new());
                }
                let models_obj = models_value
                    .as_object_mut()
                    .expect("models must be a JSON object");

                let current_model_id = self.opencode_primary_model_id();
                if let Some(original_id) = self.opencode_model_original_id.as_deref() {
                    if current_model_id.as_deref() != Some(original_id) {
                        models_obj.remove(original_id);
                    }
                }

                if let Some(model_id) = current_model_id {
                    let mut model_obj = match models_obj.remove(&model_id) {
                        Some(Value::Object(map)) => map,
                        _ => serde_json::Map::new(),
                    };
                    let model_name = self.opencode_model_name.value.trim().to_string();
                    model_obj.insert(
                        "name".to_string(),
                        json!(if model_name.is_empty() {
                            model_id.as_str()
                        } else {
                            model_name.as_str()
                        }),
                    );

                    let limit_value = model_obj
                        .entry("limit".to_string())
                        .or_insert_with(|| Value::Object(serde_json::Map::new()));
                    if !limit_value.is_object() {
                        *limit_value = Value::Object(serde_json::Map::new());
                    }
                    let limit_obj = limit_value
                        .as_object_mut()
                        .expect("limit must be a JSON object");

                    set_or_remove_u64(
                        limit_obj,
                        "context",
                        &self.opencode_model_context_limit.value,
                    );
                    set_or_remove_u64(limit_obj, "output", &self.opencode_model_output_limit.value);
                    if limit_obj.is_empty() {
                        model_obj.remove("limit");
                    }

                    models_obj.insert(model_id, Value::Object(model_obj));
                }

                if !models_obj.is_empty() {
                    settings_obj.insert("models".to_string(), models_value);
                }
            }
            AppType::OpenClaw => {
                let mut provider_value = settings_obj
                    .remove("provider")
                    .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                if !provider_value.is_object() {
                    provider_value = Value::Object(serde_json::Map::new());
                }
                let provider_obj = provider_value
                    .as_object_mut()
                    .expect("provider must be a JSON object");
                set_or_remove_trimmed(provider_obj, "apiKey", &self.opencode_api_key.value);
                set_or_remove_trimmed(provider_obj, "baseUrl", &self.opencode_base_url.value);
                provider_obj.insert("api".to_string(), json!("openai-responses"));

                let model_id = self.opencode_model_id.value.trim();
                if !model_id.is_empty() {
                    let mut model_obj = serde_json::Map::new();
                    model_obj.insert("id".to_string(), json!(model_id));
                    model_obj.insert(
                        "name".to_string(),
                        json!(if self.opencode_model_name.value.trim().is_empty() {
                            model_id
                        } else {
                            self.opencode_model_name.value.trim()
                        }),
                    );
                    model_obj.insert("reasoning".to_string(), json!(false));
                    model_obj.insert("input".to_string(), json!(["text"]));
                    model_obj.insert(
                        "cost".to_string(),
                        json!({
                            "input": 0.0,
                            "output": 0.0,
                            "cacheRead": 0.0,
                            "cacheWrite": 0.0
                        }),
                    );
                    model_obj.insert(
                        "contextWindow".to_string(),
                        json!(self
                            .opencode_model_context_limit
                            .value
                            .trim()
                            .parse::<u64>()
                            .unwrap_or(200000)),
                    );
                    model_obj.insert(
                        "maxTokens".to_string(),
                        json!(self
                            .opencode_model_output_limit
                            .value
                            .trim()
                            .parse::<u64>()
                            .unwrap_or(8192)),
                    );
                    provider_obj.insert("models".to_string(), json!([model_obj]));
                } else {
                    provider_obj.remove("models");
                }
                settings_obj.insert("provider".to_string(), provider_value);
                if !model_id.is_empty() {
                    settings_obj.insert(
                        "primaryModel".to_string(),
                        json!(format!("{}/{}", self.id.value.trim(), model_id)),
                    );
                } else {
                    settings_obj.remove("primaryModel");
                }
            }
        }

        Value::Object(provider_obj)
    }

    pub fn to_provider_json_value_with_common_config(
        &self,
        common_snippet: &str,
    ) -> Result<Value, String> {
        let mut provider_value = self.to_provider_json_value();
        if !self.include_common_config {
            return Ok(provider_value);
        }

        let snippet = common_snippet.trim();
        if snippet.is_empty() {
            return Ok(provider_value);
        }

        let Some(settings_value) = provider_value
            .as_object_mut()
            .and_then(|obj| obj.get_mut("settingsConfig"))
        else {
            return Ok(provider_value);
        };

        match self.app_type {
            AppType::Claude | AppType::Gemini => {
                let mut common: Value = serde_json::from_str(snippet).map_err(|e| {
                    crate::cli::i18n::texts::common_config_snippet_invalid_json(&e.to_string())
                })?;
                if !common.is_object() {
                    return Err(
                        crate::cli::i18n::texts::common_config_snippet_not_object().to_string()
                    );
                }

                merge_json_values(&mut common, settings_value);
                *settings_value = common;
            }
            AppType::OpenCode => {}
            AppType::OpenClaw => {}
            AppType::Codex => {
                if !settings_value.is_object() {
                    *settings_value = json!({});
                }
                let settings_obj = settings_value
                    .as_object_mut()
                    .expect("settingsConfig must be a JSON object");
                let base_config = settings_obj
                    .get("config")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                let merged_config = merge_codex_common_config_snippet(base_config, snippet)?;
                settings_obj.insert("config".to_string(), Value::String(merged_config));
            }
        }

        Ok(provider_value)
    }
}

pub(crate) fn merge_json_values(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base_obj), Value::Object(overlay_obj)) => {
            for (overlay_key, overlay_value) in overlay_obj {
                match base_obj.get_mut(overlay_key) {
                    Some(base_value) => merge_json_values(base_value, overlay_value),
                    None => {
                        base_obj.insert(overlay_key.clone(), overlay_value.clone());
                    }
                }
            }
        }
        (base_value, overlay_value) => {
            *base_value = overlay_value.clone();
        }
    }
}

pub(crate) fn strip_common_config_from_settings(
    app_type: &AppType,
    settings_value: &mut Value,
    common_snippet: &str,
) -> Result<(), String> {
    let snippet = common_snippet.trim();
    if snippet.is_empty() {
        return Ok(());
    }

    match app_type {
        AppType::Claude | AppType::Gemini => {
            let common: Value = serde_json::from_str(snippet).map_err(|e| {
                crate::cli::i18n::texts::common_config_snippet_invalid_json(&e.to_string())
            })?;
            if !common.is_object() {
                return Err(crate::cli::i18n::texts::common_config_snippet_not_object().to_string());
            }

            strip_common_json_values(settings_value, &common);
        }
        AppType::OpenCode => {}
        AppType::OpenClaw => {}
        AppType::Codex => {
            if !settings_value.is_object() {
                return Ok(());
            }
            let settings_obj = settings_value
                .as_object_mut()
                .expect("settingsConfig must be a JSON object");
            let current_config = settings_obj
                .get("config")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let stripped = strip_codex_common_config_snippet(current_config, snippet)?;
            settings_obj.insert("config".to_string(), Value::String(stripped));
        }
    }

    Ok(())
}

pub(crate) fn should_hide_provider_field(key: &str) -> bool {
    matches!(
        key,
        "category"
            | "createdAt"
            | "icon"
            | "iconColor"
            | "inFailoverQueue"
            | "meta"
            | "sortIndex"
            | "updatedAt"
    )
}

pub fn strip_provider_internal_fields(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                if should_hide_provider_field(key) {
                    continue;
                }
                out.insert(key.clone(), strip_provider_internal_fields(value));
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(strip_provider_internal_fields).collect())
        }
        other => other.clone(),
    }
}

fn upsert_optional_trimmed(obj: &mut serde_json::Map<String, Value>, key: &str, raw: &str) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        obj.remove(key);
    } else {
        obj.insert(key.to_string(), json!(trimmed));
    }
}

fn set_or_remove_trimmed(obj: &mut serde_json::Map<String, Value>, key: &str, raw: &str) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        obj.remove(key);
    } else {
        obj.insert(key.to_string(), json!(trimmed));
    }
}

fn set_or_remove_u64(obj: &mut serde_json::Map<String, Value>, key: &str, raw: &str) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        obj.remove(key);
    } else if let Ok(value) = trimmed.parse::<u64>() {
        obj.insert(key.to_string(), json!(value));
    } else {
        obj.remove(key);
    }
}

fn strip_common_json_values(target: &mut Value, common: &Value) {
    if let (Value::Object(target_obj), Value::Object(common_obj)) = (target, common) {
        let keys_to_remove = common_obj
            .iter()
            .filter_map(|(key, common_value)| {
                let Some(target_value) = target_obj.get_mut(key) else {
                    return None;
                };

                if value_matches_common(target_value, common_value) {
                    return Some(key.clone());
                }

                if target_value.is_object() && common_value.is_object() {
                    strip_common_json_values(target_value, common_value);
                    if target_value
                        .as_object()
                        .map(|obj| obj.is_empty())
                        .unwrap_or(false)
                    {
                        return Some(key.clone());
                    }
                }
                None
            })
            .collect::<Vec<_>>();

        for key in keys_to_remove {
            target_obj.remove(&key);
        }
    }
}

fn value_matches_common(value: &Value, common: &Value) -> bool {
    match (value, common) {
        (Value::Object(value_obj), Value::Object(common_obj)) => {
            value_obj.len() == common_obj.len()
                && common_obj.iter().all(|(key, common_value)| {
                    value_obj
                        .get(key)
                        .map(|value_item| value_matches_common(value_item, common_value))
                        .unwrap_or(false)
                })
        }
        (Value::Array(value_arr), Value::Array(common_arr)) => {
            value_arr.len() == common_arr.len()
                && value_arr
                    .iter()
                    .zip(common_arr.iter())
                    .all(|(value_item, common_item)| value_matches_common(value_item, common_item))
        }
        _ => value == common,
    }
}
