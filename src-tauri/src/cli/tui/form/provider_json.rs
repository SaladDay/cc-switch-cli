use crate::app_config::AppType;
use crate::services::ProviderService;
use serde_json::{json, Value};

use super::codex_config::{
    build_codex_provider_config_toml, clean_codex_provider_key, update_codex_config_snippet,
};
use super::{
    ClaudeApiFormat, GeminiAuthType, ProviderAddFormState, OPENCLAW_DEFAULT_API_PROTOCOL,
    OPENCLAW_DEFAULT_USER_AGENT,
};

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

        self.update_provider_meta(&mut provider_obj);

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
                if self.is_codex_official_provider() {
                    let existing_config = settings_obj
                        .get("config")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    let cleaned_config =
                        crate::codex_config::strip_codex_provider_config_text(existing_config)
                            .map_err(|_| ())
                            .unwrap_or_else(|_| existing_config.trim().to_string());
                    settings_obj.insert("config".to_string(), Value::String(cleaned_config));

                    let auth_value = settings_obj
                        .entry("auth".to_string())
                        .or_insert_with(|| json!({}));
                    if !auth_value.is_object() {
                        *auth_value = json!({});
                    }
                } else {
                    let provider_key =
                        clean_codex_provider_key(self.id.value.trim(), self.name.value.trim());
                    let base_url = self.codex_base_url.value.trim().trim_end_matches('/');
                    let model = if self.codex_model.is_blank() {
                        "gpt-5.4"
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
                settings_obj.remove("npm");
                settings_obj.remove("options");
                settings_obj.remove("api_key");
                settings_obj.remove("base_url");

                set_or_remove_trimmed(settings_obj, "apiKey", &self.opencode_api_key.value);
                set_or_remove_trimmed(settings_obj, "baseUrl", &self.opencode_base_url.value);

                let api_value = self.opencode_npm_package.value.trim();
                settings_obj.insert(
                    "api".to_string(),
                    json!(if api_value.is_empty() {
                        OPENCLAW_DEFAULT_API_PROTOCOL
                    } else {
                        api_value
                    }),
                );

                let mut headers_obj = match settings_obj.remove("headers") {
                    Some(Value::Object(map)) => map,
                    _ => serde_json::Map::new(),
                };
                if self.openclaw_user_agent {
                    headers_obj
                        .entry("User-Agent".to_string())
                        .or_insert_with(|| json!(OPENCLAW_DEFAULT_USER_AGENT));
                } else {
                    headers_obj.remove("User-Agent");
                }
                if headers_obj.is_empty() {
                    settings_obj.remove("headers");
                } else {
                    settings_obj.insert("headers".to_string(), Value::Object(headers_obj));
                }

                let mut models = if self.openclaw_models.is_empty() {
                    match settings_obj.remove("models") {
                        Some(Value::Array(items)) => items,
                        _ => Vec::new(),
                    }
                } else {
                    self.openclaw_models.clone()
                };

                let model_id = self.openclaw_primary_model_id();
                match model_id {
                    Some(model_id) => {
                        let mut original_index = self
                            .opencode_model_original_id
                            .as_deref()
                            .and_then(|original_id| openclaw_model_index(&models, original_id));

                        if let Some(existing_index) = openclaw_model_index(&models, &model_id) {
                            if Some(existing_index) != original_index {
                                models.remove(existing_index);
                                if let Some(index) = original_index.as_mut() {
                                    if existing_index < *index {
                                        *index = index.saturating_sub(1);
                                    }
                                }
                            }
                        }

                        let target_index =
                            original_index.or_else(|| openclaw_model_index(&models, &model_id));

                        let mut model_obj = target_index
                            .and_then(|index| models.get(index).cloned())
                            .and_then(|value| value.as_object().cloned())
                            .unwrap_or_default();

                        model_obj.insert("id".to_string(), json!(model_id.clone()));

                        let model_name = self.opencode_model_name.value.trim();
                        if model_name.is_empty() {
                            model_obj.remove("name");
                        } else {
                            model_obj.insert("name".to_string(), json!(model_name));
                        }

                        let context_value = self.opencode_model_context_limit.value.trim();
                        if context_value.is_empty() {
                            model_obj.remove("contextWindow");
                            model_obj.remove("context_window");
                        } else if let Ok(context_window) = context_value.parse::<u32>() {
                            model_obj.remove("context_window");
                            model_obj.insert("contextWindow".to_string(), json!(context_window));
                        }

                        let updated_model = Value::Object(model_obj);
                        if let Some(index) = target_index {
                            models[index] = updated_model;
                        } else {
                            models.push(updated_model);
                        }
                    }
                    None => {
                        if let Some(original_id) = self.opencode_model_original_id.as_deref() {
                            if let Some(index) = openclaw_model_index(&models, original_id) {
                                models.remove(index);
                            }
                        }
                    }
                }

                if models.is_empty() {
                    settings_obj.remove("models");
                } else {
                    settings_obj.insert("models".to_string(), Value::Array(models));
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
        let snippet = common_snippet.trim();
        if snippet.is_empty() {
            return Ok(provider_value);
        }
        if matches!(self.app_type, AppType::OpenCode | AppType::OpenClaw) {
            return Ok(provider_value);
        }

        let provider: crate::provider::Provider = serde_json::from_value(provider_value.clone())
            .map_err(|e| crate::cli::i18n::texts::tui_toast_invalid_json(&e.to_string()))?;
        let effective_settings = ProviderService::build_effective_live_snapshot(
            &self.app_type,
            &provider,
            Some(snippet),
            true,
        )
        .map_err(|e| e.to_string())?;

        if let Some(settings_value) = provider_value
            .as_object_mut()
            .and_then(|obj| obj.get_mut("settingsConfig"))
        {
            *settings_value = effective_settings;
        }

        Ok(provider_value)
    }

    fn update_provider_meta(&self, provider_obj: &mut serde_json::Map<String, Value>) {
        let should_write_common_config_meta = self.should_write_common_config_meta();
        let should_write_claude_api_format = matches!(
            self.claude_api_format,
            ClaudeApiFormat::OpenAiChat | ClaudeApiFormat::OpenAiResponses
        ) && matches!(self.app_type, AppType::Claude)
            && !self.is_claude_official_provider();

        if !should_write_common_config_meta
            && !should_write_claude_api_format
            && !provider_obj.get("meta").is_some_and(Value::is_object)
        {
            return;
        }

        let meta_value = provider_obj
            .entry("meta".to_string())
            .or_insert_with(|| json!({}));
        if !meta_value.is_object() {
            *meta_value = json!({});
        }

        let Some(meta_obj) = meta_value.as_object_mut() else {
            return;
        };

        meta_obj.remove("applyCommonConfig");
        if should_write_common_config_meta {
            meta_obj.insert(
                "commonConfigEnabled".to_string(),
                json!(if matches!(self.app_type, AppType::OpenClaw) {
                    false
                } else {
                    self.include_common_config
                }),
            );
        } else {
            meta_obj.remove("commonConfigEnabled");
        }

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

        if meta_obj.is_empty() {
            provider_obj.remove("meta");
        }
    }

    pub(crate) fn should_strip_common_config_from_applied_settings_json(&self) -> bool {
        self.include_common_config && self.should_write_common_config_meta()
    }

    fn should_write_common_config_meta(&self) -> bool {
        matches!(self.app_type, AppType::OpenClaw)
            || !self.mode.is_edit()
            || self.include_common_config_touched
            || self.has_common_config_meta()
    }

    fn has_common_config_meta(&self) -> bool {
        self.extra
            .get("meta")
            .and_then(Value::as_object)
            .is_some_and(|meta| {
                meta.contains_key("commonConfigEnabled") || meta.contains_key("applyCommonConfig")
            })
    }
}

fn openclaw_model_index(models: &[Value], model_id: &str) -> Option<usize> {
    models.iter().position(|model| {
        model
            .get("id")
            .and_then(Value::as_str)
            .map(|id| id == model_id)
            .unwrap_or(false)
    })
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
            *settings_value = ProviderService::remove_common_config_from_settings_for_preview(
                app_type,
                settings_value,
                snippet,
            )
            .map_err(|e| e.to_string())?;
        }
        AppType::OpenCode | AppType::OpenClaw => {}
        AppType::Codex => {
            *settings_value = ProviderService::remove_common_config_from_settings_for_preview(
                app_type,
                settings_value,
                snippet,
            )
            .map_err(|e| e.to_string())?;
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

#[cfg(test)]
pub(crate) fn strip_provider_internal_fields(value: &Value) -> Value {
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
