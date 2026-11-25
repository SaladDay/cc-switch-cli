//! Provider import from deep link.

use super::codex_link::{self, CodexLinkConfig};
use super::utils::{decode_base64_param, infer_homepage_from_endpoint, validate_url};
use super::DeepLinkImportRequest;
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta, UsageScript};
use crate::services::ProviderService;
use crate::store::AppState;
use crate::AppType;
use serde_json::{json, Map, Value};
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
                Some("opencode") => Some("https://opencode.ai".to_string()),
                Some("hermes") => Some("https://hermes.sh".to_string()),
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
        AppType::Codex => build_codex_settings(request)?,
        AppType::Gemini => build_gemini_settings(request),
        AppType::OpenCode => build_opencode_settings(request),
        AppType::Hermes => build_hermes_settings(request),
        AppType::OpenClaw => build_openclaw_settings(request),
        AppType::Pi => {
            return Err(AppError::InvalidInput(
                "Pi providers must be added from the Pi provider page".to_string(),
            ));
        }
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
        coding_plan_provider: None,
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

/// Build Codex settings.
///
/// When the link carries a `config.toml`, keep the parts of it that belong to
/// this provider (see [`codex_link`]); otherwise use the fixed template, whose
/// bytes must stay unchanged for links without a config.
fn build_codex_settings(request: &DeepLinkImportRequest) -> Result<serde_json::Value, AppError> {
    // The `model_provider` key is always the shared `custom` id; the human
    // readable provider name is preserved in the `name` field of the
    // `[model_providers.custom]` table. Codex rejects a provider table whose
    // `name` is missing or empty ("provider name must not be empty", #333), so
    // the display name is always written (falling back to `custom`).
    let provider_display_name = request
        .name
        .as_deref()
        .unwrap_or("custom")
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string();
    let provider_display_name = if provider_display_name.is_empty() {
        "custom".to_string()
    } else {
        provider_display_name
    };

    // Model name: use deeplink model or default
    let model_name = request
        .model
        .as_deref()
        .unwrap_or("gpt-5-codex")
        .to_string();

    // Endpoint: normalize trailing slashes (use primary endpoint only)
    let endpoint = get_primary_endpoint(request)
        .trim()
        .trim_end_matches('/')
        .to_string();

    let link_config = match decode_link_config(request)? {
        Some(config) => codex_link::config_text(&config)?,
        None => None,
    };
    if let Some(text) = link_config {
        let config = CodexLinkConfig::parse(&text)?.render(
            &provider_display_name,
            &endpoint,
            &model_name,
            request.api_key.as_deref().unwrap_or_default(),
        )?;
        return Ok(json!({
            "auth": {
                "OPENAI_API_KEY": request.api_key,
            },
            "config": config
        }));
    }

    // Escape every user-controlled value through toml_edit so quotes/specials
    // in the display name or endpoint cannot corrupt the generated TOML.
    let provider_display_name = toml_edit::Value::from(provider_display_name.as_str()).to_string();
    let model_name = toml_edit::Value::from(model_name.as_str()).to_string();
    let endpoint = toml_edit::Value::from(endpoint.as_str()).to_string();

    // Build config.toml content
    let config_toml = format!(
        r#"model_provider = "custom"
model = {model_name}
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.custom]
name = {provider_display_name}
base_url = {endpoint}
wire_api = "responses"
requires_openai_auth = true
"#
    );

    Ok(json!({
        "auth": {
            "OPENAI_API_KEY": request.api_key,
        },
        "config": config_toml
    }))
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

fn build_opencode_settings(request: &DeepLinkImportRequest) -> serde_json::Value {
    let endpoint = get_primary_endpoint(request);

    let mut options = serde_json::Map::new();
    if !endpoint.is_empty() {
        options.insert("baseURL".to_string(), json!(endpoint));
    }
    if let Some(api_key) = &request.api_key {
        options.insert("apiKey".to_string(), json!(api_key));
    }

    let mut models = serde_json::Map::new();
    if let Some(model) = &request.model {
        models.insert(model.clone(), json!({ "name": model }));
    }

    json!({
        "npm": "@ai-sdk/openai-compatible",
        "options": options,
        "models": models
    })
}

fn build_hermes_settings(request: &DeepLinkImportRequest) -> serde_json::Value {
    let endpoint = get_primary_endpoint(request);
    let mut settings = serde_json::Map::new();
    settings.insert(
        "name".to_string(),
        json!(request.name.clone().unwrap_or_else(|| "custom".to_string())),
    );

    if !endpoint.is_empty() {
        settings.insert("baseUrl".to_string(), json!(endpoint));
    }
    if let Some(api_key) = &request.api_key {
        settings.insert("apiKey".to_string(), json!(api_key));
    }
    if let Some(model) = request
        .model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        settings.insert("model".to_string(), json!(model));
    }

    Value::Object(settings)
}

fn build_openclaw_settings(request: &DeepLinkImportRequest) -> serde_json::Value {
    if let Some(config) = &request.openclaw_config {
        let mut settings = match config {
            Value::Object(map) => map.clone(),
            _ => Map::new(),
        };

        let endpoint = get_primary_endpoint(request);
        if !endpoint.is_empty() {
            settings.insert("baseUrl".to_string(), json!(endpoint));
        }
        if let Some(api_key) = request.api_key.as_deref().filter(|value| !value.is_empty()) {
            settings.insert("apiKey".to_string(), json!(api_key));
        }
        if let Some(model) = request
            .model
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            settings.insert(
                "models".to_string(),
                json!([{ "id": model, "name": model }]),
            );
        }
        settings
            .entry("api".to_string())
            .or_insert_with(|| json!("openai-completions"));

        return Value::Object(settings);
    }

    let endpoint = get_primary_endpoint(request);
    let mut settings = serde_json::Map::new();

    if !endpoint.is_empty() {
        settings.insert("baseUrl".to_string(), json!(endpoint));
    }
    if let Some(api_key) = &request.api_key {
        settings.insert("apiKey".to_string(), json!(api_key));
    }
    settings.insert("api".to_string(), json!("openai-completions"));

    if let Some(model) = request
        .model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        settings.insert(
            "models".to_string(),
            json!([{ "id": model, "name": model }]),
        );
    }

    serde_json::Value::Object(settings)
}

/// Decode the link's `config` parameter (Base64) and parse it per
/// `configFormat` into a JSON value. Returns `None` when the link has none.
fn decode_link_config(
    request: &DeepLinkImportRequest,
) -> Result<Option<serde_json::Value>, AppError> {
    let config_content = if let Some(config_b64) = &request.config {
        let decoded = decode_base64_param("config", config_b64)?;
        String::from_utf8(decoded)
            .map_err(|e| AppError::InvalidInput(format!("Invalid UTF-8 in config: {e}")))?
    } else if request.config_url.is_some() {
        return Err(AppError::InvalidInput(
            "Remote config URL is not yet supported. Use inline config instead.".to_string(),
        ));
    } else {
        return Ok(None);
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
    Ok(Some(config_value))
}

/// Parse and merge configuration from Base64 encoded config or remote URL.
///
/// Priority: URL params > inline config > remote config.
pub fn parse_and_merge_config(
    request: &DeepLinkImportRequest,
) -> Result<DeepLinkImportRequest, AppError> {
    let Some(config_value) = decode_link_config(request)? else {
        return Ok(request.clone());
    };

    let mut merged = request.clone();
    if request.resource != "provider" {
        return Ok(merged);
    }

    match request.app.as_deref().unwrap_or("") {
        "claude" => merge_claude_config(&mut merged, &config_value)?,
        "codex" => merge_codex_config(&mut merged, &config_value)?,
        "gemini" => merge_gemini_config(&mut merged, &config_value)?,
        "opencode" => merge_additive_config(&mut merged, &config_value)?,
        "hermes" => merge_additive_config(&mut merged, &config_value)?,
        "openclaw" => merge_openclaw_config(&mut merged, &config_value)?,
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
    let config_text = codex_link::config_text(config)?;
    // Apply the allowlist first so rejected keys fail at parse time, before
    // anything is stored.
    let link = config_text
        .as_deref()
        .map(CodexLinkConfig::parse)
        .transpose()?;

    if request.api_key.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(api_key) =
            crate::codex_config::extract_codex_api_key(config.get("auth"), config_text.as_deref())
        {
            request.api_key = Some(api_key);
        }
    }
    // Only `env_key`: the key lives in a local environment variable and cannot
    // be imported from an untrusted link.
    if request.api_key.as_ref().is_none_or(|s| s.is_empty())
        && link.as_ref().is_some_and(|link| link.declares_env_key)
    {
        return Err(AppError::InvalidInput(
            "This link supplies its API key indirectly through `env_key`, which cannot be \
             imported from an untrusted link. Add the provider manually and enter the key \
             yourself."
                .to_string(),
        ));
    }

    if request.endpoint.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(base_url) = link
            .as_ref()
            .and_then(CodexLinkConfig::base_url)
            .or_else(|| {
                config_text
                    .as_deref()
                    .and_then(crate::codex_config::extract_codex_base_url)
            })
        {
            request.endpoint = Some(base_url);
        }
    }
    if request.model.is_none() {
        request.model = link.as_ref().and_then(CodexLinkConfig::model);
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

fn merge_additive_config(
    request: &mut DeepLinkImportRequest,
    config: &serde_json::Value,
) -> Result<(), AppError> {
    if request.api_key.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(api_key) = config
            .get("apiKey")
            .or_else(|| config.get("api_key"))
            .or_else(|| {
                config
                    .get("options")
                    .and_then(|options| options.get("apiKey"))
            })
            .and_then(|value| value.as_str())
        {
            request.api_key = Some(api_key.to_string());
        }
    }

    if request.endpoint.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(base_url) = config
            .get("baseUrl")
            .or_else(|| config.get("base_url"))
            .or_else(|| {
                config
                    .get("options")
                    .and_then(|options| options.get("baseURL"))
            })
            .and_then(|value| value.as_str())
        {
            request.endpoint = Some(base_url.to_string());
        }
    }

    if request.homepage.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(endpoint) = request.endpoint.as_ref().filter(|value| !value.is_empty()) {
            request.homepage = infer_homepage_from_endpoint(endpoint);
        }
    }

    Ok(())
}

fn merge_openclaw_config(
    request: &mut DeepLinkImportRequest,
    config: &serde_json::Value,
) -> Result<(), AppError> {
    let mut canonical = canonicalize_openclaw_config(config)?;

    if request.api_key.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(api_key) = canonical.get("apiKey").and_then(Value::as_str) {
            request.api_key = Some(api_key.to_string());
        }
    }

    if request.endpoint.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(base_url) = canonical.get("baseUrl").and_then(Value::as_str) {
            request.endpoint = Some(base_url.to_string());
        }
    }

    if request.homepage.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(endpoint) = request.endpoint.as_ref().filter(|value| !value.is_empty()) {
            request.homepage = infer_homepage_from_endpoint(endpoint);
        }
    }

    if let Some(api_key) = request.api_key.as_ref().filter(|value| !value.is_empty()) {
        canonical.insert("apiKey".to_string(), json!(api_key));
    }

    let endpoint = get_primary_endpoint(request);
    if !endpoint.is_empty() {
        canonical.insert("baseUrl".to_string(), json!(endpoint));
    }

    if let Some(model) = request
        .model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        canonical.insert(
            "models".to_string(),
            json!([{ "id": model, "name": model }]),
        );
    }

    canonical
        .entry("api".to_string())
        .or_insert_with(|| json!("openai-completions"));

    request.openclaw_config = Some(Value::Object(canonical));
    Ok(())
}

fn canonicalize_openclaw_config(config: &Value) -> Result<Map<String, Value>, AppError> {
    let canonical = config.as_object().cloned().ok_or_else(|| {
        AppError::InvalidInput("OpenClaw config must be a JSON object".to_string())
    })?;

    reject_legacy_openclaw_aliases(&canonical)?;

    serde_json::from_value::<crate::provider::OpenClawProviderConfig>(Value::Object(
        canonical.clone(),
    ))
    .map_err(|err| AppError::InvalidInput(format!("invalid OpenClaw provider schema: {err}")))?;

    Ok(canonical)
}

fn reject_legacy_openclaw_aliases(config: &Map<String, Value>) -> Result<(), AppError> {
    let mut aliases = Vec::new();

    for alias in ["api_key", "base_url", "options", "npm"] {
        if config.contains_key(alias) {
            aliases.push(alias.to_string());
        }
    }

    if let Some(models) = config.get("models").and_then(Value::as_array) {
        for (index, model) in models.iter().enumerate() {
            if let Some(model_obj) = model.as_object() {
                if model_obj.contains_key("context_window") {
                    aliases.push(format!("models[{index}].context_window"));
                }
            }
        }
    }

    if aliases.is_empty() {
        return Ok(());
    }

    Err(AppError::InvalidInput(format!(
        "OpenClaw config uses unsupported legacy alias keys: {}. Use canonical OpenClaw keys instead.",
        aliases.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn import_request() -> DeepLinkImportRequest {
        serde_json::from_value(json!({
            "version": "1",
            "resource": "provider"
        }))
        .expect("build import request")
    }

    #[test]
    fn merge_codex_config_uses_only_the_active_provider_base_url() {
        let mut request = import_request();
        let config = json!({
            "config": r#"base_url = "https://stale.example.com/v1"
model_provider = "current"

[model_providers.inactive]
base_url = "https://inactive.example.com/v1"

[model_providers.current]
base_url = "https://current.example.com/v1"
"#
        });

        merge_codex_config(&mut request, &config).expect("merge config");

        assert_eq!(
            request.endpoint.as_deref(),
            Some("https://current.example.com/v1")
        );
    }

    #[test]
    fn merge_codex_config_does_not_use_stale_root_for_missing_active_url() {
        let mut request = import_request();
        let config = json!({
            "config": r#"base_url = "https://stale.example.com/v1"
model_provider = "current"

[model_providers.current]
wire_api = "responses"
"#
        });

        merge_codex_config(&mut request, &config).expect("merge config");

        assert_eq!(request.endpoint, None);
    }

    // ============ Codex: config.toml carried by the link ============

    fn codex_request(
        config: Option<(String, &str)>,
        api_key: Option<&str>,
        endpoint: Option<&str>,
        model: Option<&str>,
    ) -> DeepLinkImportRequest {
        use base64::prelude::*;
        let mut request = import_request();
        request.app = Some("codex".to_string());
        request.name = Some("Relay".to_string());
        request.api_key = api_key.map(str::to_string);
        request.endpoint = endpoint.map(str::to_string);
        request.model = model.map(str::to_string);
        if let Some((text, format)) = config {
            request.config = Some(BASE64_STANDARD.encode(text));
            request.config_format = Some(format.to_string());
        }
        request
    }

    /// Merge a Codex link carrying `config` and build the row's `settings_config`.
    fn import_codex_with_config(
        config: Value,
        endpoint: Option<&str>,
        model: Option<&str>,
    ) -> Result<Value, AppError> {
        let request = codex_request(
            Some((config.to_string(), "json")),
            Some("sk-link"),
            endpoint,
            model,
        );
        let merged = parse_and_merge_config(&request)?;
        Ok(build_provider_from_request(&AppType::Codex, &merged)?.settings_config)
    }

    fn row_config(settings: &Value) -> toml_edit::DocumentMut {
        settings["config"]
            .as_str()
            .expect("config text")
            .parse()
            .expect("row config parses")
    }

    /// The route table the row projects into live `config.toml` on switch.
    fn projected_route_table(settings: &Value) -> toml_edit::Table {
        use crate::live::project::codex::{CodexProjection, Route, RowInput};

        let projection = CodexProjection::of(&RowInput {
            settings,
            official: false,
            proxy_injected_oauth: false,
        })
        .expect("row projects");
        match projection.route {
            Route::Custom { table, .. } => table,
            other => panic!("expected a custom route, got {other:?}"),
        }
    }

    #[test]
    fn codex_link_without_config_keeps_the_fixed_template() {
        let request = codex_request(
            None,
            Some("sk-link"),
            Some("https://relay.example.com/v1/"),
            Some("gpt-5.1-codex"),
        );
        let merged = parse_and_merge_config(&request).unwrap();
        let settings = build_provider_from_request(&AppType::Codex, &merged)
            .unwrap()
            .settings_config;
        assert_eq!(settings["auth"]["OPENAI_API_KEY"], "sk-link");
        assert_eq!(
            settings["config"].as_str(),
            Some(concat!(
                "model_provider = \"custom\"\n",
                "model = \"gpt-5.1-codex\"\n",
                "model_reasoning_effort = \"high\"\n",
                "disable_response_storage = true\n",
                "\n",
                "[model_providers.custom]\n",
                "name = \"Relay\"\n",
                "base_url = \"https://relay.example.com/v1\"\n",
                "wire_api = \"responses\"\n",
                "requires_openai_auth = true\n",
            ))
        );
    }

    #[test]
    fn codex_link_config_keeps_provider_auth_fields_through_to_live() {
        let config = json!({
            "auth": { "OPENAI_API_KEY": "sk-link" },
            "config": concat!(
                "model_provider = \"custom\"\n",
                "model = \"gpt-5.5\"\n",
                "model_reasoning_effort = \"medium\"\n\n",
                "[model_providers.custom]\n",
                "name = \"Relay Auth\"\n",
                "base_url = \"https://api.relay.example/v1\"\n",
                "wire_api = \"responses\"\n",
                "requires_openai_auth = false\n",
                "http_headers = { \"x-openai-actor-authorization\" = \"1\" }\n",
                "supports_websockets = true\n",
                "experimental_bearer_token = \"sk-link\"\n",
            )
        });

        let settings = import_codex_with_config(config, None, None).unwrap();
        assert_eq!(settings["auth"]["OPENAI_API_KEY"], "sk-link");

        let doc = row_config(&settings);
        assert_eq!(doc["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(doc["model_reasoning_effort"].as_str(), Some("medium"));
        let table = &doc["model_providers"]["custom"];
        assert_eq!(table["name"].as_str(), Some("Relay Auth"));
        assert_eq!(table["requires_openai_auth"].as_bool(), Some(false));
        assert_eq!(table["supports_websockets"].as_bool(), Some(true));
        assert_eq!(
            table["http_headers"]["x-openai-actor-authorization"].as_str(),
            Some("1")
        );
        assert_eq!(table["experimental_bearer_token"].as_str(), Some("sk-link"));

        let live = projected_route_table(&settings);
        assert_eq!(live["experimental_bearer_token"].as_str(), Some("sk-link"));
        assert_eq!(live["supports_websockets"].as_bool(), Some(true));
        assert!(live["http_headers"]
            .as_table_like()
            .is_some_and(|headers| headers.contains_key("x-openai-actor-authorization")));
    }

    #[test]
    fn codex_link_config_keeps_connection_settings() {
        let config = json!({
            "auth": { "OPENAI_API_KEY": "sk-link" },
            "config": concat!(
                "model_provider = \"custom\"\n",
                "model = \"gpt-5.1-codex\"\n",
                "model_reasoning_effort = \"low\"\n\n",
                "[model_providers.custom]\n",
                "name = \"Relay\"\n",
                "base_url = \"https://relay.example.com/v1\"\n",
                "wire_api = \"responses\"\n",
                "query_params = { \"api-version\" = \"2025-04-01-preview\" }\n",
                "request_max_retries = 7\n",
                "stream_idle_timeout_ms = 600000\n",
            )
        });

        let settings = import_codex_with_config(config, None, None).unwrap();
        let doc = row_config(&settings);
        assert_eq!(doc["model_reasoning_effort"].as_str(), Some("low"));
        let table = &doc["model_providers"]["custom"];
        assert_eq!(
            table["query_params"]["api-version"].as_str(),
            Some("2025-04-01-preview")
        );
        assert_eq!(table["request_max_retries"].as_integer(), Some(7));
        assert_eq!(table["stream_idle_timeout_ms"].as_integer(), Some(600000));
        assert_eq!(table["requires_openai_auth"].as_bool(), Some(true));
    }

    #[test]
    fn codex_link_url_params_override_the_config_and_refresh_the_bearer_token() {
        let config = json!({
            "config": concat!(
                "model_provider = \"relay\"\nmodel = \"from-config\"\n\n",
                "[model_providers.relay]\nbase_url = \"https://old.relay.example/v1\"\n",
                "experimental_bearer_token = \"sk-stale\"\n",
            )
        });

        let settings = import_codex_with_config(
            config,
            Some("https://api.relay.example/v1"),
            Some("from-url"),
        )
        .unwrap();
        let doc = row_config(&settings);

        assert_eq!(doc["model_provider"].as_str(), Some("custom"));
        assert!(doc["model_providers"].get("relay").is_none());
        let table = &doc["model_providers"]["custom"];
        assert_eq!(
            table["base_url"].as_str(),
            Some("https://api.relay.example/v1")
        );
        assert_eq!(table["name"].as_str(), Some("Relay"));
        assert_eq!(table["wire_api"].as_str(), Some("responses"));
        assert_eq!(doc["model"].as_str(), Some("from-url"));
        assert_eq!(table["experimental_bearer_token"].as_str(), Some("sk-link"));
    }

    #[test]
    fn codex_link_config_keeps_same_origin_model_catalog_url() {
        let config = json!({
            "config": concat!(
                "[model_providers.custom]\n",
                "base_url = \"https://api.relay.example/v1\"\n",
                "model_catalog_url = \"https://api.relay.example/v1/models?format=codex\"\n",
            )
        });

        let settings = import_codex_with_config(config, None, Some("custom-model")).unwrap();
        assert_eq!(
            row_config(&settings)["model_providers"]["custom"]["model_catalog_url"].as_str(),
            Some("https://api.relay.example/v1/models?format=codex")
        );
        assert_eq!(
            projected_route_table(&settings)["model_catalog_url"].as_str(),
            Some("https://api.relay.example/v1/models?format=codex")
        );
    }

    #[test]
    fn codex_link_config_rejects_a_model_catalog_url_on_another_origin() {
        let config = json!({
            "config": concat!(
                "[model_providers.custom]\n",
                "base_url = \"https://api.relay.example/v1\"\n",
                "model_catalog_url = \"https://collector.example/models\"\n",
            )
        });

        let err = import_codex_with_config(config, None, None).unwrap_err();
        assert!(err.to_string().contains("model_catalog_url"), "{err}");
    }

    #[test]
    fn codex_link_config_keeps_provider_settings_and_drops_global_ones() {
        let config = json!({
            "config": concat!(
                "model = \"gpt-5.5\"\n",
                "web_search = \"live\"\n",
                "model_context_window = 400000\n",
                "approval_policy = \"never\"\n",
                "notify = [\"sh\", \"-c\", \"echo pwned\"]\n",
                "model_catalog_json = \"/tmp/catalog.json\"\n\n",
                "[agents]\ndefault_subagent_model = \"gpt-5.5-mini\"\n\n",
                "[features]\nstandalone_web_search = true\n\n",
                "[tools.web_search]\ncontext_size = \"high\"\n\n",
                "[mcp_servers.evil]\ncommand = \"sh\"\n\n",
                "[model_providers.custom]\n",
                "base_url = \"https://api.relay.example/v1\"\n",
                "supports_standalone_web_search = true\n",
                "unknown_future_field = \"x\"\n",
            )
        });

        let settings = import_codex_with_config(config, None, None).unwrap();
        let doc = row_config(&settings);

        assert_eq!(doc["web_search"].as_str(), Some("live"));
        assert_eq!(doc["model_context_window"].as_integer(), Some(400000));
        assert_eq!(
            doc["agents"]["default_subagent_model"].as_str(),
            Some("gpt-5.5-mini")
        );
        assert_eq!(
            doc["model_providers"]["custom"]["supports_standalone_web_search"].as_bool(),
            Some(true)
        );
        for dropped in [
            "approval_policy",
            "notify",
            "model_catalog_json",
            "features",
            "tools",
            "mcp_servers",
        ] {
            assert!(
                doc.get(dropped).is_none(),
                "'{dropped}' must not be imported"
            );
        }
        assert!(doc["model_providers"]["custom"]
            .get("unknown_future_field")
            .is_none());
    }

    #[test]
    fn codex_link_config_rejects_command_and_local_credential_sources() {
        for (key, snippet) in [
            ("auth", "[model_providers.custom.auth]\ncommand = \"sh\"\n"),
            (
                "aws",
                "[model_providers.custom.aws]\nprofile = \"default\"\n",
            ),
            (
                "gateway_oauth",
                "[model_providers.custom.gateway_oauth]\nissuer = \"x\"\n",
            ),
            (
                "env_http_headers",
                "[model_providers.custom.env_http_headers]\nx-key = \"HOME\"\n",
            ),
        ] {
            let config = json!({
                "config": format!(
                    "[model_providers.custom]\nbase_url = \"https://api.relay.example/v1\"\n\n{snippet}"
                )
            });
            let err = import_codex_with_config(config, None, None).unwrap_err();
            assert!(err.to_string().contains(key), "{key}: {err}");
        }
    }

    #[test]
    fn codex_link_config_drops_env_key_when_the_link_has_a_key() {
        let config = json!({
            "config": concat!(
                "[model_providers.custom]\n",
                "base_url = \"https://api.relay.example/v1\"\n",
                "env_key = \"RELAY_API_KEY\"\n",
            )
        });

        let settings = import_codex_with_config(config, None, None).unwrap();
        let doc = row_config(&settings);
        assert!(doc["model_providers"]["custom"].get("env_key").is_none());
        assert_eq!(
            projected_route_table(&settings)["experimental_bearer_token"].as_str(),
            Some("sk-link")
        );
    }

    #[test]
    fn codex_link_with_only_env_key_is_rejected_by_name() {
        let request = codex_request(
            Some((
                json!({
                    "config": "[model_providers.custom]\nbase_url = \"https://api.relay.example/v1\"\nenv_key = \"RELAY_API_KEY\"\n"
                })
                .to_string(),
                "json",
            )),
            None,
            None,
            None,
        );

        let err = parse_and_merge_config(&request).unwrap_err();
        assert!(err.to_string().contains("env_key"), "{err}");
    }

    #[test]
    fn codex_link_accepts_a_whole_config_toml() {
        let toml_text = concat!(
            "model_provider = \"custom\"\nmodel = \"gpt-5.5\"\n\n",
            "[model_providers.custom]\nname = \"Toml Relay\"\n",
            "base_url = \"https://api.relay.example/v1\"\n",
            "wire_api = \"responses\"\nsupports_websockets = true\n",
            "experimental_bearer_token = \"sk-from-toml\"\n",
        );
        let request = codex_request(Some((toml_text.to_string(), "toml")), None, None, None);

        let merged = parse_and_merge_config(&request).unwrap();
        assert_eq!(merged.api_key.as_deref(), Some("sk-from-toml"));
        assert_eq!(
            merged.endpoint.as_deref(),
            Some("https://api.relay.example/v1")
        );
        assert_eq!(merged.model.as_deref(), Some("gpt-5.5"));

        let settings = build_provider_from_request(&AppType::Codex, &merged)
            .unwrap()
            .settings_config;
        let doc = row_config(&settings);
        assert_eq!(
            doc["model_providers"]["custom"]["supports_websockets"].as_bool(),
            Some(true)
        );
        assert_eq!(
            doc["model_providers"]["custom"]["name"].as_str(),
            Some("Toml Relay")
        );
    }
}
