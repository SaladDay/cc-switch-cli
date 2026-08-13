use crate::app_config::AppType;
use crate::claude_model_config::{
    set_claude_role_model, CLAUDE_DEFAULT_MODEL_ENV_KEY, CLAUDE_LEGACY_SMALL_FAST_MODEL_ENV_KEY,
};
use crate::cli::i18n::texts;
use crate::provider::{ClaudeApiKeyField, CodexChatReasoningConfig};
use crate::provider_preset_models::CODEX_DEFAULT_MODEL;
use crate::services::ProviderService;
use serde_json::{json, Value};
use std::collections::HashSet;

use super::codex_config::{build_codex_third_party_config_toml, update_codex_config_snippet};
use super::{
    normalize_local_proxy_header_overrides, parse_codex_model_catalog_context_window,
    ClaudeApiFormat, ClaudeModelRole, GeminiAuthType, PromptCacheRoutingMode, ProviderAddFormState,
    UsageQueryTemplate, OPENCLAW_DEFAULT_API_PROTOCOL, OPENCLAW_DEFAULT_USER_AGENT,
};

pub(crate) fn normalize_gemini_common_config_for_form(snippet: &str) -> Result<Value, String> {
    let snippet = snippet.trim();
    if snippet.is_empty() {
        return Ok(json!({}));
    }

    let Value::Object(entries) = serde_json::from_str::<Value>(snippet)
        .map_err(|error| texts::common_config_snippet_invalid_json(&error.to_string()))?
    else {
        return Err(texts::common_config_snippet_not_object().to_string());
    };

    let forbidden_keys = entries
        .keys()
        .filter(|key| {
            key.as_str() == "GOOGLE_GEMINI_BASE_URL"
                || key.as_str() == "GEMINI_API_KEY"
                || ProviderService::is_sensitive_common_config_key_for_preview(key)
        })
        .cloned()
        .collect::<Vec<_>>();
    if !forbidden_keys.is_empty() {
        return Err(texts::gemini_common_config_invalid_keys(
            &forbidden_keys.join(", "),
        ));
    }

    let mut env = serde_json::Map::new();
    for (key, value) in entries {
        let Some(value) = value.as_str() else {
            return Err(texts::gemini_common_config_invalid_values().to_string());
        };
        let value = value.trim();
        if !value.is_empty() {
            env.insert(key, Value::String(value.to_string()));
        }
    }
    Ok(Value::Object(env))
}

const CLAUDE_COMMON_CONFIG_FORBIDDEN_KEYS: [&str; 3] = ["__proto__", "constructor", "prototype"];

fn sanitize_claude_common_config_value_for_form(value: Value) -> Value {
    match value {
        Value::Object(entries) => Value::Object(
            entries
                .into_iter()
                .filter(|(key, _)| !CLAUDE_COMMON_CONFIG_FORBIDDEN_KEYS.contains(&key.as_str()))
                .map(|(key, value)| (key, sanitize_claude_common_config_value_for_form(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(sanitize_claude_common_config_value_for_form)
                .collect(),
        ),
        value => value,
    }
}

pub(crate) fn sanitize_claude_common_config_for_form(snippet: &str) -> Result<Value, String> {
    let value = serde_json::from_str::<Value>(snippet)
        .map_err(|error| texts::common_config_snippet_invalid_json(&error.to_string()))?;
    if !value.is_object() {
        return Err(texts::common_config_snippet_not_object().to_string());
    }
    Ok(sanitize_claude_common_config_value_for_form(value))
}

fn json_value_is_subset_for_form(target: &Value, source: &Value) -> bool {
    match source {
        Value::Object(source_map) => {
            let Some(target_map) = target.as_object() else {
                return false;
            };
            source_map.iter().all(|(key, source_value)| {
                target_map.get(key).is_some_and(|target_value| {
                    json_value_is_subset_for_form(target_value, source_value)
                })
            })
        }
        Value::Array(source_array) => {
            let Some(target_array) = target.as_array() else {
                return false;
            };
            target_array.len() == source_array.len()
                && target_array
                    .iter()
                    .zip(source_array)
                    .all(|(target_value, source_value)| {
                        json_value_is_subset_for_form(target_value, source_value)
                    })
        }
        _ => target == source,
    }
}

fn json_deep_remove_for_form(target: &mut Value, source: &Value) {
    let (Some(target_map), Some(source_map)) = (target.as_object_mut(), source.as_object()) else {
        return;
    };

    for (key, source_value) in source_map {
        let mut remove_key = false;
        if let Some(target_value) = target_map.get_mut(key) {
            if source_value.is_object() && target_value.is_object() {
                json_deep_remove_for_form(target_value, source_value);
                remove_key = target_value
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty);
            } else if json_value_is_subset_for_form(target_value, source_value) {
                // Upstream's form utility treats arrays as atomic values: it
                // removes the field only when length, order, and values all
                // match, instead of deleting matching array members.
                remove_key = true;
            }
        }
        if remove_key {
            target_map.remove(key);
        }
    }
}

pub(crate) fn claude_settings_contain_common_config_for_form(
    settings: &Value,
    common_snippet: &str,
) -> bool {
    sanitize_claude_common_config_for_form(common_snippet)
        .ok()
        .filter(|source| source.as_object().is_some_and(|object| !object.is_empty()))
        .is_some_and(|source| json_value_is_subset_for_form(settings, &source))
}

fn sanitize_toml_common_config_value_for_form(value: toml::Value) -> toml::Value {
    match value {
        toml::Value::Table(entries) => toml::Value::Table(
            entries
                .into_iter()
                .filter(|(key, _)| !CLAUDE_COMMON_CONFIG_FORBIDDEN_KEYS.contains(&key.as_str()))
                .map(|(key, value)| (key, sanitize_toml_common_config_value_for_form(value)))
                .collect(),
        ),
        toml::Value::Array(items) => toml::Value::Array(
            items
                .into_iter()
                .map(sanitize_toml_common_config_value_for_form)
                .collect(),
        ),
        value => value,
    }
}

fn toml_value_is_subset_for_form(target: &toml::Value, source: &toml::Value) -> bool {
    match source {
        toml::Value::Table(source_table) => {
            let Some(target_table) = target.as_table() else {
                return false;
            };
            source_table.iter().all(|(key, source_value)| {
                target_table.get(key).is_some_and(|target_value| {
                    toml_value_is_subset_for_form(target_value, source_value)
                })
            })
        }
        toml::Value::Array(source_array) => {
            let Some(target_array) = target.as_array() else {
                return false;
            };
            target_array.len() == source_array.len()
                && target_array
                    .iter()
                    .zip(source_array)
                    .all(|(target_value, source_value)| {
                        toml_value_is_subset_for_form(target_value, source_value)
                    })
        }
        _ => target == source,
    }
}

pub(crate) fn codex_settings_contain_common_config_for_form(
    settings: &Value,
    common_snippet: &str,
) -> bool {
    let config = settings
        .get("config")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match (
        toml::from_str::<toml::Value>(config),
        toml::from_str::<toml::Value>(common_snippet),
    ) {
        (Ok(target), Ok(source)) => {
            let source = sanitize_toml_common_config_value_for_form(source);
            source.as_table().is_some_and(|table| !table.is_empty())
                && toml_value_is_subset_for_form(&target, &source)
        }
        _ => {
            // Match the upstream UI's malformed-TOML fallback.
            let target = config.split_whitespace().collect::<Vec<_>>().join(" ");
            let source = common_snippet
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            !source.is_empty() && target.contains(&source)
        }
    }
}

impl ProviderAddFormState {
    pub(crate) fn existing_codex_config_text(&self) -> &str {
        self.extra
            .get("settingsConfig")
            .and_then(Value::as_object)
            .and_then(|settings| settings.get("config"))
            .and_then(Value::as_str)
            .unwrap_or("")
    }

    pub(crate) fn effective_codex_config_text(&self) -> String {
        if self.is_codex_official_provider() {
            return self.effective_official_codex_config_text();
        }

        let fallback_model = if self.codex_model.is_blank() {
            CODEX_DEFAULT_MODEL
        } else {
            self.codex_model.value.trim()
        };
        let model = if self.codex_local_routing_enabled() {
            self.codex_model_catalog
                .iter()
                .map(|row| row.model.trim())
                .find(|model| !model.is_empty())
                .unwrap_or(fallback_model)
        } else {
            fallback_model
        };
        self.effective_custom_codex_config_text(model)
    }

    pub(crate) fn effective_codex_config_text_with_common_config(
        &self,
        common_snippet: &str,
    ) -> Result<String, String> {
        let config = self.effective_codex_config_text();
        if !self.include_common_config || common_snippet.trim().is_empty() {
            return Ok(config);
        }

        let settings = json!({ "config": config });
        let effective = ProviderService::apply_common_config_to_settings_for_preview(
            &AppType::Codex,
            &settings,
            common_snippet,
        )
        .map_err(|err| err.to_string())?;

        Ok(effective
            .get("config")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    fn codex_config_and_model_catalog_for_save(&self) -> (String, Vec<Value>) {
        if self.is_codex_official_provider() {
            return (self.effective_official_codex_config_text(), Vec::new());
        }

        let model_catalog = if self.codex_local_routing_enabled() {
            self.normalized_codex_model_catalog_for_save()
        } else {
            Vec::new()
        };
        let model = if !model_catalog.is_empty() {
            model_catalog[0]["model"]
                .as_str()
                .unwrap_or(CODEX_DEFAULT_MODEL)
        } else if self.codex_model.is_blank() {
            CODEX_DEFAULT_MODEL
        } else {
            self.codex_model.value.trim()
        };
        (
            self.effective_custom_codex_config_text(model),
            model_catalog,
        )
    }

    fn effective_official_codex_config_text(&self) -> String {
        let existing_config = self.existing_codex_config_text();
        let mut config = crate::codex_config::strip_codex_provider_config_text(existing_config)
            .map_err(|_| ())
            .unwrap_or_else(|_| existing_config.trim().to_string());
        if self.codex_goal_mode_touched {
            config = crate::codex_config::set_codex_goal_mode(&config, self.codex_goal_mode);
        }
        config
    }

    fn effective_custom_codex_config_text(&self, model: &str) -> String {
        let existing_config = self.existing_codex_config_text();
        let base_url = self.codex_base_url.value.trim().trim_end_matches('/');
        let base_config = if existing_config.trim().is_empty() {
            build_codex_third_party_config_toml(
                self.name.value.trim(),
                base_url,
                model,
                self.codex_wire_api,
            )
        } else {
            existing_config.to_string()
        };
        let mut config = update_codex_config_snippet(
            &base_config,
            base_url,
            model,
            self.codex_wire_api,
            self.codex_requires_openai_auth,
            self.codex_env_key.value.trim(),
        );
        if self.codex_goal_mode_touched {
            config = crate::codex_config::set_codex_goal_mode(&config, self.codex_goal_mode);
        }
        if self.codex_remote_compaction_touched {
            config = crate::codex_config::set_codex_remote_compaction(
                &config,
                self.codex_remote_compaction,
                self.name.value.trim(),
            );
        }
        config
    }

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
                if self.is_claude_codex_oauth_provider() {
                    env_obj.remove("ANTHROPIC_AUTH_TOKEN");
                    env_obj.remove("ANTHROPIC_API_KEY");
                    env_obj.insert(
                        "ANTHROPIC_BASE_URL".to_string(),
                        json!("https://chatgpt.com/backend-api/codex"),
                    );
                } else {
                    set_or_remove_trimmed(
                        env_obj,
                        self.claude_api_key_field.as_env_key(),
                        &self.claude_api_key.value,
                    );
                    env_obj.remove(self.claude_api_key_field.alternate_env_key());
                    set_or_remove_trimmed(
                        env_obj,
                        "ANTHROPIC_BASE_URL",
                        &self.claude_base_url.value,
                    );
                }
                if self.claude_fallback_model_touched {
                    set_or_remove_trimmed(
                        env_obj,
                        CLAUDE_DEFAULT_MODEL_ENV_KEY,
                        &self.claude_model.value,
                    );
                }
                for role in ClaudeModelRole::ALL {
                    if self.claude_model_role_was_touched(role.index()) {
                        let model = self.claude_model_value_for_config(role.index());
                        set_claude_role_model(env_obj, role, &model);
                    }
                }
                if self.claude_fallback_model_touched || self.any_claude_model_role_was_touched() {
                    env_obj.remove(CLAUDE_LEGACY_SMALL_FAST_MODEL_ENV_KEY);
                }
                if self.claude_teammates_touched {
                    if self.claude_teammates {
                        env_obj.insert(
                            "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS".to_string(),
                            json!("1"),
                        );
                    } else {
                        env_obj.remove("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS");
                    }
                }
                if self.claude_tool_search_touched {
                    if self.claude_tool_search {
                        env_obj.insert("ENABLE_TOOL_SEARCH".to_string(), json!("true"));
                    } else {
                        env_obj.remove("ENABLE_TOOL_SEARCH");
                    }
                }
                if self.claude_effort_max_touched {
                    if self.claude_effort_max {
                        env_obj.insert("CLAUDE_CODE_EFFORT_LEVEL".to_string(), json!("max"));
                    } else {
                        env_obj.remove("CLAUDE_CODE_EFFORT_LEVEL");
                    }
                }
                if self.claude_disable_auto_upgrade_touched {
                    if self.claude_disable_auto_upgrade {
                        env_obj.insert("DISABLE_AUTOUPDATER".to_string(), json!("1"));
                    } else {
                        env_obj.remove("DISABLE_AUTOUPDATER");
                    }
                }
                settings_obj.remove("api_format");
                settings_obj.remove("apiFormat");
                settings_obj.remove("openrouter_compat_mode");
                if self.claude_hide_attribution && self.claude_hide_attribution_touched {
                    settings_obj.insert(
                        "attribution".to_string(),
                        json!({
                            "commit": "",
                            "pr": ""
                        }),
                    );
                } else if self.claude_hide_attribution_touched {
                    settings_obj.remove("attribution");
                }
            }
            AppType::Codex => {
                let (config_toml, model_catalog) = self.codex_config_and_model_catalog_for_save();
                settings_obj.insert("config".to_string(), Value::String(config_toml));
                if self.is_codex_official_provider() {
                    let auth_value = settings_obj
                        .entry("auth".to_string())
                        .or_insert_with(|| json!({}));
                    if !auth_value.is_object() {
                        *auth_value = json!({});
                    }
                    settings_obj.remove("modelCatalog");
                } else {
                    if !model_catalog.is_empty() {
                        settings_obj.insert(
                            "modelCatalog".to_string(),
                            json!({ "models": model_catalog }),
                        );
                    } else {
                        settings_obj.remove("modelCatalog");
                    }

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
                    let mut model_obj = match models_obj.get(&model_id) {
                        Some(Value::Object(map)) => map.clone(),
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
            AppType::Hermes => {
                for legacy_key in ["api", "apiKey", "apiMode", "baseUrl", "baseURL", "endpoint"] {
                    settings_obj.remove(legacy_key);
                }

                settings_obj.insert("api_mode".to_string(), json!(self.hermes_api_mode_value()));

                let base_url = self
                    .hermes_base_url
                    .value
                    .trim()
                    .trim_end_matches('/')
                    .to_string();
                set_or_remove_trimmed(settings_obj, "base_url", &base_url);
                set_or_remove_trimmed(settings_obj, "api_key", &self.hermes_api_key.value);

                if self.hermes_models.is_empty() {
                    settings_obj.remove("models");
                } else {
                    settings_obj.insert(
                        "models".to_string(),
                        Value::Array(self.hermes_models.clone()),
                    );
                }

                set_or_remove_f64(
                    settings_obj,
                    "rate_limit_delay",
                    &self.hermes_rate_limit_delay.value,
                );
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
        let raw_snippet = common_snippet.trim();
        if raw_snippet.is_empty()
            || !self.include_common_config
            || !ProviderAddFormState::supports_common_config(&self.app_type)
        {
            return Ok(provider_value);
        }

        let normalized_gemini_snippet = if matches!(self.app_type, AppType::Gemini) {
            let normalized = normalize_gemini_common_config_for_form(raw_snippet)?;
            if normalized.as_object().is_none_or(serde_json::Map::is_empty) {
                return Ok(provider_value);
            }
            Some(
                serde_json::to_string(&normalized)
                    .map_err(|error| texts::failed_to_serialize_json(&error.to_string()))?,
            )
        } else {
            None
        };
        let snippet = normalized_gemini_snippet.as_deref().unwrap_or(raw_snippet);
        ProviderService::validate_common_config_snippet_for_preview(&self.app_type, snippet)
            .map_err(|e| e.to_string())?;

        let normalized_claude_snippet = if matches!(self.app_type, AppType::Claude) {
            Some(
                serde_json::to_string(&sanitize_claude_common_config_for_form(snippet)?)
                    .map_err(|error| texts::failed_to_serialize_json(&error.to_string()))?,
            )
        } else {
            None
        };
        let snippet = normalized_claude_snippet.as_deref().unwrap_or(snippet);
        let settings = provider_value
            .get("settingsConfig")
            .cloned()
            .unwrap_or_else(|| json!({}));
        // A form preview applies only the common snippet. The durable live
        // snapshot builder also performs app-specific file reconciliation
        // (notably reading Gemini settings.json), which upstream form hooks do
        // not do and which must never leak into the provider's raw draft.
        let effective_settings = ProviderService::apply_common_config_to_settings_for_preview(
            &self.app_type,
            &settings,
            snippet,
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
            ClaudeApiFormat::OpenAiChat
                | ClaudeApiFormat::OpenAiResponses
                | ClaudeApiFormat::GeminiNative
        ) && matches!(self.app_type, AppType::Claude)
            && !self.is_claude_official_provider();
        let should_write_codex_api_format =
            matches!(self.app_type, AppType::Codex) && !self.is_codex_official_provider();
        let should_write_prompt_cache_routing = should_write_codex_api_format
            && self.codex_is_chat_format()
            && !matches!(
                self.codex_prompt_cache_routing,
                PromptCacheRoutingMode::Auto
            );
        let should_write_claude_api_key_field = matches!(self.app_type, AppType::Claude)
            && !self.is_claude_official_provider()
            && !self.is_claude_codex_oauth_provider()
            && self.claude_api_key_field == ClaudeApiKeyField::ApiKey;
        let is_codex_oauth = self.is_claude_codex_oauth_provider();
        let local_proxy_body_override = self
            .local_proxy_body_override
            .as_ref()
            .filter(|value| value.as_object().is_none_or(|object| !object.is_empty()));
        let should_write_local_proxy_settings = self.supports_local_proxy_settings()
            && (!self.custom_user_agent.is_blank()
                || !self.local_proxy_header_overrides.is_empty()
                || local_proxy_body_override.is_some());
        let should_write_full_url = self.supports_full_url_mode() && self.is_full_url;

        if !should_write_common_config_meta
            && !should_write_claude_api_format
            && !should_write_codex_api_format
            && !should_write_prompt_cache_routing
            && !should_write_claude_api_key_field
            && !is_codex_oauth
            && !should_write_local_proxy_settings
            && !self.has_usage_script_meta()
            && !self.usage_query_touched
            && !should_write_full_url
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
                json!(self.include_common_config),
            );
        } else {
            meta_obj.remove("commonConfigEnabled");
        }

        if matches!(self.app_type, AppType::Claude) {
            match self.claude_api_format {
                _ if self.is_claude_official_provider() && !is_codex_oauth => {
                    meta_obj.remove("apiFormat");
                }
                ClaudeApiFormat::Anthropic if is_codex_oauth => {
                    meta_obj.insert("apiFormat".to_string(), json!("openai_responses"));
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
                ClaudeApiFormat::GeminiNative => {
                    meta_obj.insert("apiFormat".to_string(), json!("gemini_native"));
                }
            }

            if should_write_claude_api_key_field {
                meta_obj.insert(
                    "apiKeyField".to_string(),
                    json!(self.claude_api_key_field.as_env_key()),
                );
            } else {
                meta_obj.remove("apiKeyField");
            }
        }

        if matches!(self.app_type, AppType::Codex) {
            if self.is_codex_official_provider() {
                meta_obj.remove("apiFormat");
                meta_obj.remove("apiKeyField");
                meta_obj.remove("impersonateClaudeCode");
                meta_obj.remove("maxOutputTokens");
                meta_obj.remove("codexChatReasoning");
                meta_obj.remove("promptCacheRouting");
            } else {
                let api_format = match self.claude_api_format {
                    ClaudeApiFormat::OpenAiChat => "openai_chat",
                    ClaudeApiFormat::Anthropic => "anthropic",
                    _ => "openai_responses",
                };
                meta_obj.insert("apiFormat".to_string(), json!(api_format));
                if matches!(self.claude_api_format, ClaudeApiFormat::Anthropic) {
                    if self.claude_api_key_field == ClaudeApiKeyField::ApiKey {
                        meta_obj.insert(
                            "apiKeyField".to_string(),
                            json!(self.claude_api_key_field.as_env_key()),
                        );
                    } else {
                        meta_obj.remove("apiKeyField");
                    }
                    if self.codex_impersonate_claude_code {
                        meta_obj.insert("impersonateClaudeCode".to_string(), json!(true));
                    } else {
                        meta_obj.remove("impersonateClaudeCode");
                    }
                    let max_output_tokens = self
                        .codex_max_output_tokens
                        .value
                        .trim()
                        .parse::<u64>()
                        .ok();
                    if let Some(value) = max_output_tokens.filter(|value| *value > 0) {
                        meta_obj.insert("maxOutputTokens".to_string(), json!(value));
                    } else {
                        meta_obj.remove("maxOutputTokens");
                    }
                } else {
                    meta_obj.remove("apiKeyField");
                    meta_obj.remove("impersonateClaudeCode");
                    meta_obj.remove("maxOutputTokens");
                }
                // Reasoning capability is persisted only when routing is enabled
                // AND the upstream format is Chat (reasoning is Chat-only).
                if self.codex_local_routing_enabled() && self.codex_is_chat_format() {
                    if let Some(reasoning) =
                        normalize_codex_chat_reasoning_for_save(&self.codex_chat_reasoning)
                    {
                        meta_obj.insert("codexChatReasoning".to_string(), reasoning);
                    } else {
                        meta_obj.remove("codexChatReasoning");
                    }
                } else {
                    meta_obj.remove("codexChatReasoning");
                }

                if should_write_prompt_cache_routing {
                    meta_obj.insert(
                        "promptCacheRouting".to_string(),
                        json!(self.codex_prompt_cache_routing.as_str()),
                    );
                } else {
                    meta_obj.remove("promptCacheRouting");
                }
            }
        }

        if matches!(self.app_type, AppType::Claude | AppType::Codex) {
            if should_write_full_url {
                meta_obj.insert("isFullUrl".to_string(), json!(true));
            } else {
                meta_obj.remove("isFullUrl");
            }
        }

        if self.supports_local_proxy_settings() {
            let custom_user_agent = self.custom_user_agent.value.trim();
            if custom_user_agent.is_empty() {
                meta_obj.remove("customUserAgent");
            } else {
                meta_obj.insert("customUserAgent".to_string(), json!(custom_user_agent));
            }

            let mut overrides = serde_json::Map::new();
            if !self.local_proxy_header_overrides.is_empty() {
                let headers = normalize_local_proxy_header_overrides(
                    self.local_proxy_header_overrides
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone())),
                )
                .unwrap_or_else(|_| self.local_proxy_header_overrides.clone());
                overrides.insert("headers".to_string(), json!(headers));
            }
            if let Some(body) = local_proxy_body_override {
                overrides.insert("body".to_string(), body.clone());
            }
            if overrides.is_empty() {
                meta_obj.remove("localProxyRequestOverrides");
            } else {
                meta_obj.insert(
                    "localProxyRequestOverrides".to_string(),
                    Value::Object(overrides),
                );
            }
        } else if !self.is_claude_github_copilot_provider() {
            meta_obj.remove("customUserAgent");
            meta_obj.remove("localProxyRequestOverrides");
        }

        if is_codex_oauth {
            meta_obj.insert("providerType".to_string(), json!("codex_oauth"));
            meta_obj.insert(
                "authBinding".to_string(),
                json!({
                    "source": "managed_account",
                    "authProvider": "codex_oauth",
                    "accountId": self.codex_oauth_account_id.as_deref(),
                }),
            );
            if self.codex_oauth_account_id.is_none() {
                if let Some(auth_binding) = meta_obj
                    .get_mut("authBinding")
                    .and_then(|value| value.as_object_mut())
                {
                    auth_binding.remove("accountId");
                }
            }
            meta_obj.insert("codexFastMode".to_string(), json!(self.codex_fast_mode));
        } else {
            if meta_obj
                .get("providerType")
                .and_then(Value::as_str)
                .is_some_and(|value| value == "codex_oauth")
            {
                meta_obj.remove("providerType");
            }
            if meta_obj
                .get("authBinding")
                .and_then(|value| value.get("authProvider"))
                .and_then(Value::as_str)
                .is_some_and(|value| value == "codex_oauth")
            {
                meta_obj.remove("authBinding");
            }
            meta_obj.remove("codexFastMode");
        }

        self.update_usage_script_meta(meta_obj);

        if meta_obj.is_empty() {
            provider_obj.remove("meta");
        }
    }

    fn update_usage_script_meta(&self, meta_obj: &mut serde_json::Map<String, Value>) {
        // Provider edits outside the Usage Query page must not normalize or
        // rewrite a shared desktop configuration.
        if !self.usage_query_touched {
            return;
        }

        let mut script = meta_obj
            .get("usage_script")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        script.insert("enabled".to_string(), json!(self.usage_query_enabled));
        script.insert("language".to_string(), json!("javascript"));
        script.insert("code".to_string(), json!(self.usage_query_code.as_str()));
        script.insert(
            "timeout".to_string(),
            json!(normalize_usage_timeout(&self.usage_query_timeout.value)),
        );
        script.insert(
            "templateType".to_string(),
            json!(self.usage_query_template.as_str()),
        );
        script.insert(
            "autoQueryInterval".to_string(),
            json!(normalize_usage_interval(
                &self.usage_query_auto_interval.value
            )),
        );

        match self.usage_query_template {
            UsageQueryTemplate::General => {
                set_or_remove_trimmed(&mut script, "apiKey", &self.usage_query_api_key.value);
                set_or_remove_trimmed(&mut script, "baseUrl", &self.usage_query_base_url.value);
                script.remove("accessToken");
                script.remove("userId");
            }
            UsageQueryTemplate::NewApi => {
                set_or_remove_trimmed(&mut script, "baseUrl", &self.usage_query_base_url.value);
                set_or_remove_trimmed(
                    &mut script,
                    "accessToken",
                    &self.usage_query_access_token.value,
                );
                set_or_remove_trimmed(&mut script, "userId", &self.usage_query_user_id.value);
                script.remove("apiKey");
            }
            UsageQueryTemplate::TokenPlan => {
                for key in ["apiKey", "baseUrl", "accessToken", "userId"] {
                    script.remove(key);
                }
                set_or_remove_trimmed(
                    &mut script,
                    "codingPlanProvider",
                    &self.usage_query_coding_plan_provider.value,
                );
            }
            UsageQueryTemplate::OfficialSubscription => {
                for key in ["apiKey", "baseUrl", "accessToken", "userId"] {
                    script.remove(key);
                }
            }
            UsageQueryTemplate::Custom
            | UsageQueryTemplate::GitHubCopilot
            | UsageQueryTemplate::Balance => {
                for key in ["apiKey", "baseUrl", "accessToken", "userId"] {
                    script.remove(key);
                }
            }
        }

        meta_obj.insert("usage_script".to_string(), Value::Object(script));
    }

    fn should_write_common_config_meta(&self) -> bool {
        ProviderAddFormState::supports_common_config(&self.app_type)
            && (!self.mode.is_edit()
                || self.include_common_config_touched
                || self.has_common_config_meta())
    }

    fn has_common_config_meta(&self) -> bool {
        self.extra
            .get("meta")
            .and_then(Value::as_object)
            .is_some_and(|meta| {
                meta.contains_key("commonConfigEnabled") || meta.contains_key("applyCommonConfig")
            })
    }

    pub(super) fn has_usage_script_meta(&self) -> bool {
        self.extra
            .get("meta")
            .and_then(Value::as_object)
            .is_some_and(|meta| meta.contains_key("usage_script"))
    }

    fn normalized_codex_model_catalog_for_save(&self) -> Vec<Value> {
        let mut seen = HashSet::new();
        let mut models = Vec::new();

        for row in &self.codex_model_catalog {
            let model = row.model.trim();
            if model.is_empty() || !seen.insert(model.to_string()) {
                continue;
            }

            let mut obj = serde_json::Map::new();
            obj.insert("model".to_string(), json!(model));

            let display_name = row.display_name.trim();
            if !display_name.is_empty() {
                obj.insert("displayName".to_string(), json!(display_name));
            }

            if let Some(context_window) =
                parse_codex_model_catalog_context_window(row.context_window.trim())
            {
                if context_window > 0 {
                    obj.insert("contextWindow".to_string(), json!(context_window));
                }
            }

            if let Some(supports_parallel_tool_calls) = row.supports_parallel_tool_calls {
                obj.insert(
                    "supportsParallelToolCalls".to_string(),
                    json!(supports_parallel_tool_calls),
                );
            }

            let input_modalities = row
                .input_modalities
                .iter()
                .filter(|modality| !modality.trim().is_empty())
                .collect::<Vec<_>>();
            if !input_modalities.is_empty() {
                obj.insert("inputModalities".to_string(), json!(input_modalities));
            }

            let base_instructions = row.base_instructions.trim();
            if !base_instructions.is_empty() {
                obj.insert("baseInstructions".to_string(), json!(base_instructions));
            }

            models.push(Value::Object(obj));
        }

        models
    }
}

fn normalize_codex_chat_reasoning_for_save(value: &CodexChatReasoningConfig) -> Option<Value> {
    let raw = serde_json::to_value(value).ok()?;
    let has_explicit_config = raw.as_object().is_some_and(|obj| !obj.is_empty());
    let supports_effort = value.supports_effort == Some(true);
    let supports_thinking = value.supports_thinking == Some(true) || supports_effort;

    if !supports_thinking && !supports_effort {
        return has_explicit_config.then(|| {
            json!({
                "supportsThinking": false,
                "supportsEffort": false,
                "thinkingParam": "none",
                "effortParam": "none",
                "outputFormat": value.output_format.as_deref().unwrap_or("auto"),
            })
        });
    }

    let mut obj = serde_json::Map::new();
    obj.insert("supportsThinking".to_string(), json!(supports_thinking));
    obj.insert("supportsEffort".to_string(), json!(supports_effort));
    obj.insert(
        "thinkingParam".to_string(),
        json!(if supports_thinking {
            value.thinking_param.as_deref().unwrap_or("thinking")
        } else {
            "none"
        }),
    );
    obj.insert(
        "effortParam".to_string(),
        json!(if supports_effort {
            value.effort_param.as_deref().unwrap_or("reasoning_effort")
        } else {
            "none"
        }),
    );
    if supports_effort {
        obj.insert(
            "effortValueMode".to_string(),
            json!(value.effort_value_mode.as_deref().unwrap_or("passthrough")),
        );
    }
    obj.insert(
        "outputFormat".to_string(),
        json!(value.output_format.as_deref().unwrap_or("auto")),
    );

    Some(Value::Object(obj))
}

pub(crate) fn normalize_usage_timeout(raw: &str) -> u64 {
    normalize_usage_number(raw, 10, None)
}

pub(crate) fn normalize_usage_interval(raw: &str) -> u64 {
    normalize_usage_number(raw, 0, Some(1440))
}

fn normalize_usage_number(raw: &str, fallback: u64, max: Option<u64>) -> u64 {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return fallback;
    }

    let Ok(value) = trimmed.parse::<f64>() else {
        return fallback;
    };
    if !value.is_finite() || value < 0.0 {
        return fallback;
    }

    let floored = value.floor();
    let capped = max
        .map(|max| floored.min(max as f64))
        .unwrap_or(floored)
        .max(0.0);
    capped as u64
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
    let raw_snippet = common_snippet.trim();
    let normalized_gemini_snippet = if matches!(app_type, AppType::Gemini) {
        let normalized = normalize_gemini_common_config_for_form(raw_snippet)?;
        if normalized.as_object().is_none_or(serde_json::Map::is_empty) {
            return Ok(());
        }
        Some(
            serde_json::to_string(&normalized)
                .map_err(|error| texts::failed_to_serialize_json(&error.to_string()))?,
        )
    } else {
        None
    };
    let snippet = normalized_gemini_snippet.as_deref().unwrap_or(raw_snippet);
    if snippet.is_empty() {
        return Ok(());
    }

    match app_type {
        AppType::Claude => {
            let source = sanitize_claude_common_config_for_form(snippet)?;
            json_deep_remove_for_form(settings_value, &source);
        }
        AppType::Gemini => {
            *settings_value = ProviderService::remove_common_config_from_settings_for_preview(
                app_type,
                settings_value,
                snippet,
            )
            .map_err(|e| e.to_string())?;
        }
        AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => {}
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

pub(crate) fn claude_hide_attribution_enabled(settings_config: &Value) -> bool {
    let Some(attribution) = settings_config
        .get("attribution")
        .and_then(Value::as_object)
    else {
        return false;
    };

    attribution.get("commit").and_then(Value::as_str) == Some("")
        && attribution.get("pr").and_then(Value::as_str) == Some("")
}

pub(crate) fn claude_teammates_enabled(settings_config: &Value) -> bool {
    let Some(value) = settings_config
        .get("env")
        .and_then(|env| env.get("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS"))
    else {
        return false;
    };
    value.as_str() == Some("1") || value.as_i64() == Some(1)
}

pub(crate) fn claude_tool_search_enabled(settings_config: &Value) -> bool {
    let Some(value) = settings_config
        .get("env")
        .and_then(|env| env.get("ENABLE_TOOL_SEARCH"))
    else {
        return false;
    };
    value.as_str() == Some("true") || value.as_str() == Some("1")
}

pub(crate) fn claude_effort_max_enabled(settings_config: &Value) -> bool {
    settings_config
        .get("env")
        .and_then(|env| env.get("CLAUDE_CODE_EFFORT_LEVEL"))
        .and_then(Value::as_str)
        == Some("max")
}

pub(crate) fn claude_disable_auto_upgrade_enabled(settings_config: &Value) -> bool {
    let Some(value) = settings_config
        .get("env")
        .and_then(|env| env.get("DISABLE_AUTOUPDATER"))
    else {
        return false;
    };
    value.as_str() == Some("1") || value.as_i64() == Some(1)
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

fn set_or_remove_f64(obj: &mut serde_json::Map<String, Value>, key: &str, raw: &str) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        obj.remove(key);
    } else if let Ok(value) = trimmed.parse::<f64>() {
        if value.is_finite() && value >= 0.0 {
            obj.insert(key.to_string(), json!(value));
        } else {
            obj.remove(key);
        }
    } else {
        obj.remove(key);
    }
}
