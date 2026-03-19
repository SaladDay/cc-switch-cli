//! Deep link URL parser.
//!
//! Parses `ccswitch://` URLs into `DeepLinkImportRequest` structures.

use super::utils::validate_url;
use super::DeepLinkImportRequest;
use crate::error::AppError;
use std::collections::HashMap;
use url::Url;

/// Parse a `ccswitch://` URL into a `DeepLinkImportRequest`.
///
/// Expected format:
/// `ccswitch://v1/import?resource=provider&...`
pub fn parse_deeplink_url(url_str: &str) -> Result<DeepLinkImportRequest, AppError> {
    let url = Url::parse(url_str)
        .map_err(|e| AppError::InvalidInput(format!("Invalid deep link URL: {e}")))?;

    let scheme = url.scheme();
    if scheme != "ccswitch" {
        return Err(AppError::InvalidInput(format!(
            "Invalid scheme: expected 'ccswitch', got '{scheme}'"
        )));
    }

    let version = url
        .host_str()
        .ok_or_else(|| AppError::InvalidInput("Missing version in URL host".to_string()))?
        .to_string();
    if version != "v1" {
        return Err(AppError::InvalidInput(format!(
            "Unsupported protocol version: {version}"
        )));
    }

    let path = url.path();
    if path != "/import" {
        return Err(AppError::InvalidInput(format!(
            "Invalid path: expected '/import', got '{path}'"
        )));
    }

    let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
    let resource = params
        .get("resource")
        .ok_or_else(|| AppError::InvalidInput("Missing 'resource' parameter".to_string()))?
        .clone();

    match resource.as_str() {
        "provider" => parse_provider_deeplink(&params, version, resource),
        _ => Err(AppError::InvalidInput(format!(
            "Unsupported resource type: {resource}"
        ))),
    }
}

fn parse_provider_deeplink(
    params: &HashMap<String, String>,
    version: String,
    resource: String,
) -> Result<DeepLinkImportRequest, AppError> {
    let app = params
        .get("app")
        .ok_or_else(|| AppError::InvalidInput("Missing 'app' parameter".to_string()))?
        .clone();

    if app != "claude"
        && app != "codex"
        && app != "gemini"
        && app != "opencode"
        && app != "openclaw"
    {
        return Err(AppError::InvalidInput(format!(
            "Invalid app type: must be 'claude', 'codex', 'gemini', 'opencode', or 'openclaw', got '{app}'"
        )));
    }

    let name = params
        .get("name")
        .ok_or_else(|| AppError::InvalidInput("Missing 'name' parameter".to_string()))?
        .clone();

    let homepage = params.get("homepage").cloned();
    let endpoint = params.get("endpoint").cloned();
    let api_key = params.get("apiKey").cloned();

    if let Some(ref hp) = homepage {
        if !hp.is_empty() {
            validate_url(hp, "homepage")?;
        }
    }

    if let Some(ref ep) = endpoint {
        for (i, url) in ep.split(',').enumerate() {
            let trimmed = url.trim();
            if !trimmed.is_empty() {
                validate_url(trimmed, &format!("endpoint[{i}]"))?;
            }
        }
    }

    Ok(DeepLinkImportRequest {
        version,
        resource,
        app: Some(app),
        name: Some(name),
        enabled: params.get("enabled").and_then(|v| v.parse::<bool>().ok()),
        homepage,
        endpoint,
        api_key,
        icon: params
            .get("icon")
            .map(|v| v.trim().to_lowercase())
            .filter(|v| !v.is_empty()),
        model: params.get("model").cloned(),
        notes: params.get("notes").cloned(),
        haiku_model: params.get("haikuModel").cloned(),
        sonnet_model: params.get("sonnetModel").cloned(),
        opus_model: params.get("opusModel").cloned(),
        content: None,
        description: None,
        apps: None,
        repo: None,
        directory: None,
        branch: None,
        config: params.get("config").cloned(),
        config_format: params.get("configFormat").cloned(),
        config_url: params.get("configUrl").cloned(),
        usage_enabled: params
            .get("usageEnabled")
            .and_then(|v| v.parse::<bool>().ok()),
        usage_script: params.get("usageScript").cloned(),
        usage_api_key: params.get("usageApiKey").cloned(),
        usage_base_url: params.get("usageBaseUrl").cloned(),
        usage_access_token: params.get("usageAccessToken").cloned(),
        usage_user_id: params.get("usageUserId").cloned(),
        usage_auto_interval: params
            .get("usageAutoInterval")
            .and_then(|v| v.parse::<u64>().ok()),
    })
}
