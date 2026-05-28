use reqwest::RequestBuilder;

use crate::{provider::Provider, proxy::error::ProxyError};

use super::{AuthInfo, AuthStrategy, ProviderAdapter};

pub struct GeminiAdapter;

#[derive(Debug, Clone)]
pub struct OAuthCredentials {
    pub access_token: String,
}

impl GeminiAdapter {
    pub fn new() -> Self {
        Self
    }

    fn is_oauth_provider(&self, provider: &Provider) -> bool {
        self.extract_key_raw(provider)
            .map(|key| key.starts_with("ya29.") || key.starts_with('{'))
            .unwrap_or(false)
    }

    fn extract_key_raw(&self, provider: &Provider) -> Option<String> {
        if let Some(env) = provider.settings_config.get("env") {
            if let Some(key) = env.get("GEMINI_API_KEY").and_then(|v| v.as_str()) {
                return Some(key.to_string());
            }
        }

        provider
            .settings_config
            .get("apiKey")
            .or_else(|| provider.settings_config.get("api_key"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn parse_oauth_credentials(&self, key: &str) -> Option<OAuthCredentials> {
        if key.starts_with("ya29.") {
            return Some(OAuthCredentials {
                access_token: key.to_string(),
            });
        }

        if !key.starts_with('{') {
            return None;
        }

        let json = serde_json::from_str::<serde_json::Value>(key).ok()?;
        let access_token = json
            .get("access_token")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let refresh_token = json
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if access_token.is_empty() && refresh_token.is_none() {
            return None;
        }

        Some(OAuthCredentials { access_token })
    }
}

impl Default for GeminiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for GeminiAdapter {
    fn name(&self) -> &'static str {
        "Gemini"
    }

    fn extract_base_url(&self, provider: &Provider) -> Result<String, ProxyError> {
        if let Some(env) = provider.settings_config.get("env") {
            if let Some(url) = env.get("GOOGLE_GEMINI_BASE_URL").and_then(|v| v.as_str()) {
                return Ok(url.trim_end_matches('/').to_string());
            }
            if let Some(url) = env.get("GEMINI_BASE_URL").and_then(|v| v.as_str()) {
                return Ok(url.trim_end_matches('/').to_string());
            }
            if let Some(url) = env.get("BASE_URL").and_then(|v| v.as_str()) {
                return Ok(url.trim_end_matches('/').to_string());
            }
        }

        if let Some(url) = provider
            .settings_config
            .get("base_url")
            .and_then(|v| v.as_str())
        {
            return Ok(url.trim_end_matches('/').to_string());
        }

        if let Some(url) = provider
            .settings_config
            .get("baseURL")
            .and_then(|v| v.as_str())
        {
            return Ok(url.trim_end_matches('/').to_string());
        }

        Err(ProxyError::ConfigError(
            "Gemini Provider 缺少 base_url 配置".to_string(),
        ))
    }

    fn extract_auth(&self, provider: &Provider) -> Option<AuthInfo> {
        let key = self.extract_key_raw(provider)?;
        if self.is_oauth_provider(provider) {
            if let Some(creds) = self.parse_oauth_credentials(&key) {
                return Some(AuthInfo::with_access_token(key, creds.access_token));
            }
        }

        Some(AuthInfo::new(key, AuthStrategy::Google))
    }

    fn build_url(&self, base_url: &str, endpoint: &str) -> String {
        let base_trimmed = base_url.trim_end_matches('/');
        let endpoint_trimmed = endpoint.trim_start_matches('/');
        let mut url = format!("{base_trimmed}/{endpoint_trimmed}");

        for pattern in ["/v1beta", "/v1"] {
            let duplicate = format!("{pattern}{pattern}");
            if url.contains(&duplicate) {
                url = url.replace(&duplicate, pattern);
            }
        }

        url
    }

    fn add_auth_headers(&self, request: RequestBuilder, auth: &AuthInfo) -> RequestBuilder {
        match auth.strategy {
            AuthStrategy::GoogleOAuth => {
                let token = auth.access_token.as_ref().unwrap_or(&auth.api_key);
                request
                    .header("Authorization", format!("Bearer {token}"))
                    .header("x-goog-api-client", "GeminiCLI/1.0")
            }
            _ => request.header("x-goog-api-key", &auth.api_key),
        }
    }
}
