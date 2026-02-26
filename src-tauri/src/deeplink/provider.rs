//! Provider import from deep link.

use super::utils::{decode_base64_param, infer_homepage_from_endpoint, validate_url};
use super::DeepLinkImportRequest;
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta, UsageScript};
use crate::services::ProviderService;
use crate::store::AppState;
use crate::AppType;
use serde_json::json;
use std::str::FromStr;

/// Import a provider from a deep link request.
pub fn import_provider_from_deeplink(
    state: &AppState,
    request: DeepLinkImportRequest,
) -> Result<String, AppError> {
    if request.resource != "provider" {
        return Err(AppError::InvalidInput(format!(
            "Expected provider resource, got '{}'",
            request.resource
        )));
    }

    let mut merged_request = parse_and_merge_config(&request)?;

    let app_str = merged_request
        .app
        .clone()
        .ok_or_else(|| AppError::InvalidInput("Missing 'app' field for provider".to_string()))?;

    let api_key = merged_request.api_key.as_ref().ok_or_else(|| {
        AppError::InvalidInput("API key is required (either in URL or config file)".to_string())
    })?;
    if api_key.is_empty() {
        return Err(AppError::InvalidInput(
            "API key cannot be empty".to_string(),
        ));
    }

    let endpoint_str = merged_request.endpoint.as_ref().ok_or_else(|| {
        AppError::InvalidInput("Endpoint is required (either in URL or config file)".to_string())
    })?;
    let all_endpoints: Vec<String> = endpoint_str
        .split(',')
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect();
    let primary_endpoint = all_endpoints
        .first()
        .ok_or_else(|| AppError::InvalidInput("Endpoint cannot be empty".to_string()))?;

    for (i, endpoint) in all_endpoints.iter().enumerate() {
        validate_url(endpoint, &format!("endpoint[{i}]"))?;
    }

    if merged_request
        .homepage
        .as_ref()
        .is_none_or(|s| s.is_empty())
    {
        merged_request.homepage = infer_homepage_from_endpoint(primary_endpoint);

        if merged_request.homepage.is_none() {
            merged_request.homepage = match merged_request.app.as_deref() {
                Some("claude") => Some("https://anthropic.com".to_string()),
                Some("codex") => Some("https://openai.com".to_string()),
                Some("gemini") => Some("https://ai.google.dev".to_string()),
                _ => None,
            };
        }
    }

    let homepage = merged_request.homepage.as_ref().ok_or_else(|| {
        AppError::InvalidInput("Homepage is required (either in URL or config file)".to_string())
    })?;
    if homepage.is_empty() {
        return Err(AppError::InvalidInput(
            "Homepage cannot be empty".to_string(),
        ));
    }
    validate_url(homepage, "homepage")?;

    let name = merged_request
        .name
        .clone()
        .ok_or_else(|| AppError::InvalidInput("Missing 'name' field for provider".to_string()))?;

    let app_type = AppType::from_str(&app_str)
        .map_err(|_| AppError::InvalidInput(format!("Invalid app type: {app_str}")))?;

    let mut provider = build_provider_from_request(&app_type, &merged_request)?;

    // Add extra endpoints as custom endpoints (skip first one as it's the primary)
    if all_endpoints.len() > 1 {
        let meta = provider.meta.get_or_insert_with(ProviderMeta::default);
        for endpoint in all_endpoints.iter().skip(1) {
            let normalized = endpoint.trim().trim_end_matches('/').to_string();
            if normalized.is_empty() {
                continue;
            }
            meta.custom_endpoints.insert(
                normalized.clone(),
                crate::settings::CustomEndpoint {
                    url: normalized,
                    added_at: chrono::Utc::now().timestamp_millis(),
                    last_used: None,
                },
            );
        }
    }

    // Generate a stable-ish provider id: `{sanitized_name}-{timestamp_ms}`
    let timestamp = chrono::Utc::now().timestamp_millis();
    let sanitized_name = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>()
        .to_lowercase();
    provider.id = format!("{sanitized_name}-{timestamp}");
    let provider_id = provider.id.clone();

    ProviderService::add(state, app_type.clone(), provider)?;

    if merged_request.enabled == Some(true) {
        ProviderService::switch(state, app_type, &provider_id)?;
    }

    Ok(provider_id)
}

fn build_provider_from_request(
    app_type: &AppType,
    request: &DeepLinkImportRequest,
) -> Result<Provider, AppError> {
    let settings_config = match app_type {
        AppType::Claude => build_claude_settings(request),
        AppType::Codex => build_codex_settings(request),
        AppType::Gemini => build_gemini_settings(request),
    };

    let meta = build_provider_meta(request)?;

    Ok(Provider {
        id: String::new(), // generated by caller
        name: request.name.clone().unwrap_or_default(),
        settings_config,
        website_url: request.homepage.clone(),
        category: None,
        created_at: Some(chrono::Utc::now().timestamp_millis()),
        sort_index: None,
        notes: request.notes.clone(),
        meta,
        icon: request.icon.clone(),
        icon_color: None,
        in_failover_queue: false,
    })
}

fn get_primary_endpoint(request: &DeepLinkImportRequest) -> String {
    request
        .endpoint
        .as_ref()
        .and_then(|ep| ep.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn build_provider_meta(request: &DeepLinkImportRequest) -> Result<Option<ProviderMeta>, AppError> {
    if request.usage_script.is_none()
        && request.usage_enabled.is_none()
        && request.usage_api_key.is_none()
        && request.usage_base_url.is_none()
        && request.usage_access_token.is_none()
        && request.usage_user_id.is_none()
        && request.usage_auto_interval.is_none()
    {
        return Ok(None);
    }

    let code = if let Some(script_b64) = &request.usage_script {
        let decoded = decode_base64_param("usage_script", script_b64)?;
        String::from_utf8(decoded)
            .map_err(|e| AppError::InvalidInput(format!("Invalid UTF-8 in usage_script: {e}")))?
    } else {
        String::new()
    };

    let enabled = request.usage_enabled.unwrap_or(!code.is_empty());

    let usage_script = UsageScript {
        enabled,
        language: "javascript".to_string(),
        code,
        timeout: Some(10),
        api_key: request
            .usage_api_key
            .clone()
            .or_else(|| request.api_key.clone()),
        base_url: request.usage_base_url.clone().or_else(|| {
            let primary = get_primary_endpoint(request);
            if primary.is_empty() {
                None
            } else {
                Some(primary)
            }
        }),
        access_token: request.usage_access_token.clone(),
        user_id: request.usage_user_id.clone(),
        template_type: None,
        auto_query_interval: request.usage_auto_interval,
    };

    Ok(Some(ProviderMeta {
        usage_script: Some(usage_script),
        ..Default::default()
    }))
}

fn build_claude_settings(request: &DeepLinkImportRequest) -> serde_json::Value {
    let mut env = serde_json::Map::new();
    env.insert(
        "ANTHROPIC_AUTH_TOKEN".to_string(),
        json!(request.api_key.clone().unwrap_or_default()),
    );
    env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        json!(get_primary_endpoint(request)),
    );

    if let Some(model) = &request.model {
        env.insert("ANTHROPIC_MODEL".to_string(), json!(model));
    }
    if let Some(haiku_model) = &request.haiku_model {
        env.insert(
            "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
            json!(haiku_model),
        );
    }
    if let Some(sonnet_model) = &request.sonnet_model {
        env.insert(
            "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
            json!(sonnet_model),
        );
    }
    if let Some(opus_model) = &request.opus_model {
        env.insert(
            "ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(),
            json!(opus_model),
        );
    }

    json!({ "env": env })
}

fn build_codex_settings(request: &DeepLinkImportRequest) -> serde_json::Value {
    let model_name = request
        .model
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("gpt-5.2-codex");

    let endpoint = get_primary_endpoint(request)
        .trim()
        .trim_end_matches('/')
        .to_string();

    // Generate a provider key from the name (same logic as clean_codex_provider_key)
    let provider_key = {
        let raw = request.name.as_deref().unwrap_or("custom").trim();
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
    };

    // Use upstream model_provider + [model_providers.<key>] format
    let config_snippet = format!(
        "model_provider = \"{provider_key}\"\n\
         model = \"{model_name}\"\n\
         \n\
         [model_providers.{provider_key}]\n\
         base_url = \"{endpoint}\"\n\
         wire_api = \"responses\"\n\
         env_key = \"OPENAI_API_KEY\""
    );

    json!({
        "auth": {
            "OPENAI_API_KEY": request.api_key,
        },
        "config": config_snippet
    })
}

fn build_gemini_settings(request: &DeepLinkImportRequest) -> serde_json::Value {
    let mut env = serde_json::Map::new();
    env.insert("GEMINI_API_KEY".to_string(), json!(request.api_key));
    env.insert(
        "GOOGLE_GEMINI_BASE_URL".to_string(),
        json!(get_primary_endpoint(request)),
    );
    if let Some(model) = &request.model {
        env.insert("GEMINI_MODEL".to_string(), json!(model));
    }
    json!({ "env": env })
}

/// Parse and merge configuration from Base64 encoded config or remote URL.
///
/// Priority: URL params > inline config > remote config.
pub fn parse_and_merge_config(
    request: &DeepLinkImportRequest,
) -> Result<DeepLinkImportRequest, AppError> {
    if request.config.is_none() && request.config_url.is_none() {
        return Ok(request.clone());
    }

    let config_content = if let Some(config_b64) = &request.config {
        let decoded = decode_base64_param("config", config_b64)?;
        String::from_utf8(decoded)
            .map_err(|e| AppError::InvalidInput(format!("Invalid UTF-8 in config: {e}")))?
    } else if request.config_url.is_some() {
        return Err(AppError::InvalidInput(
            "Remote config URL is not yet supported. Use inline config instead.".to_string(),
        ));
    } else {
        return Ok(request.clone());
    };

    let format = request.config_format.as_deref().unwrap_or("json");
    let config_value: serde_json::Value = match format {
        "json" => serde_json::from_str(&config_content)
            .map_err(|e| AppError::InvalidInput(format!("Invalid JSON config: {e}")))?,
        "toml" => {
            let toml_value: toml::Value = toml::from_str(&config_content)
                .map_err(|e| AppError::InvalidInput(format!("Invalid TOML config: {e}")))?;
            serde_json::to_value(toml_value)
                .map_err(|e| AppError::Message(format!("Failed to convert TOML to JSON: {e}")))?
        }
        _ => {
            return Err(AppError::InvalidInput(format!(
                "Unsupported config format: {format}"
            )))
        }
    };

    let mut merged = request.clone();
    if request.resource != "provider" {
        return Ok(merged);
    }

    match request.app.as_deref().unwrap_or("") {
        "claude" => merge_claude_config(&mut merged, &config_value)?,
        "codex" => merge_codex_config(&mut merged, &config_value)?,
        "gemini" => merge_gemini_config(&mut merged, &config_value)?,
        "" => return Ok(merged),
        other => return Err(AppError::InvalidInput(format!("Invalid app type: {other}"))),
    }

    Ok(merged)
}

fn merge_claude_config(
    request: &mut DeepLinkImportRequest,
    config: &serde_json::Value,
) -> Result<(), AppError> {
    let env = config
        .get("env")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            AppError::InvalidInput("Claude config must have 'env' object".to_string())
        })?;

    if request.api_key.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(token) = env.get("ANTHROPIC_AUTH_TOKEN").and_then(|v| v.as_str()) {
            request.api_key = Some(token.to_string());
        }
    }

    if request.endpoint.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(base_url) = env.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) {
            request.endpoint = Some(base_url.to_string());
        }
    }

    if request.homepage.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(endpoint) = request.endpoint.as_ref().filter(|s| !s.is_empty()) {
            request.homepage = infer_homepage_from_endpoint(endpoint);
            if request.homepage.is_none() {
                request.homepage = Some("https://anthropic.com".to_string());
            }
        }
    }

    if request.model.is_none() {
        request.model = env
            .get("ANTHROPIC_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }
    if request.haiku_model.is_none() {
        request.haiku_model = env
            .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }
    if request.sonnet_model.is_none() {
        request.sonnet_model = env
            .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }
    if request.opus_model.is_none() {
        request.opus_model = env
            .get("ANTHROPIC_DEFAULT_OPUS_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    Ok(())
}

fn merge_codex_config(
    request: &mut DeepLinkImportRequest,
    config: &serde_json::Value,
) -> Result<(), AppError> {
    if request.api_key.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(api_key) = config
            .get("auth")
            .and_then(|v| v.get("OPENAI_API_KEY"))
            .and_then(|v| v.as_str())
        {
            request.api_key = Some(api_key.to_string());
        }
    }

    if let Some(config_str) = config.get("config").and_then(|v| v.as_str()) {
        if let Ok(toml_value) = toml::from_str::<toml::Value>(config_str) {
            if request.endpoint.as_ref().is_none_or(|s| s.is_empty()) {
                if let Some(base_url) = extract_codex_base_url(&toml_value) {
                    request.endpoint = Some(base_url);
                }
            }
            if request.model.is_none() {
                if let Some(model) = toml_value.get("model").and_then(|v| v.as_str()) {
                    request.model = Some(model.to_string());
                }
            }
        }
    }

    if request.homepage.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(endpoint) = request.endpoint.as_ref().filter(|s| !s.is_empty()) {
            request.homepage = infer_homepage_from_endpoint(endpoint);
            if request.homepage.is_none() {
                request.homepage = Some("https://openai.com".to_string());
            }
        }
    }

    Ok(())
}

fn merge_gemini_config(
    request: &mut DeepLinkImportRequest,
    config: &serde_json::Value,
) -> Result<(), AppError> {
    let env = match config.get("env").and_then(|v| v.as_object()) {
        Some(obj) => Some(obj),
        None => config.as_object(),
    };

    if let Some(env) = env {
        if request.api_key.as_ref().is_none_or(|s| s.is_empty()) {
            if let Some(api_key) = env.get("GEMINI_API_KEY").and_then(|v| v.as_str()) {
                request.api_key = Some(api_key.to_string());
            }
        }

        if request.endpoint.as_ref().is_none_or(|s| s.is_empty()) {
            if let Some(base_url) = env
                .get("GOOGLE_GEMINI_BASE_URL")
                .or_else(|| env.get("GEMINI_BASE_URL"))
                .and_then(|v| v.as_str())
            {
                request.endpoint = Some(base_url.to_string());
            }
        }

        if request.model.is_none() {
            request.model = env
                .get("GEMINI_MODEL")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }

    if request.homepage.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(endpoint) = request.endpoint.as_ref().filter(|s| !s.is_empty()) {
            request.homepage = infer_homepage_from_endpoint(endpoint);
            if request.homepage.is_none() {
                request.homepage = Some("https://ai.google.dev".to_string());
            }
        }
    }

    Ok(())
}

fn extract_codex_base_url(toml_value: &toml::Value) -> Option<String> {
    // CLI edition stores base_url in root snippet.
    if let Some(base_url) = toml_value.get("base_url").and_then(|v| v.as_str()) {
        if !base_url.trim().is_empty() {
            return Some(base_url.to_string());
        }
    }

    if let Some(providers) = toml_value.get("model_providers").and_then(|v| v.as_table()) {
        for (_key, provider) in providers.iter() {
            if let Some(base_url) = provider.get("base_url").and_then(|v| v.as_str()) {
                return Some(base_url.to_string());
            }
        }
    }
    None
}
